use anyhow::{Context, Result};
use aws_lc_rs::{
    kem::{Ciphertext, DecapsulationKey, EncapsulationKey},
    kem::{ML_KEM_1024}
};
use dotenvy::dotenv;
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_client::types::update::Update;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use proc_exit::{Code, exit};
use std::env;
use std::process::Command;
use std::result::Result::Ok;
use std::sync::Arc;
use tokio::{select, signal};
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;


use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, fmt::time::LocalTime};

use tg_anti_spam_bot::handle_update;

const SESSION_FILE: &str = "bot.session";

#[tokio::main]
async fn main() -> Result<()> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(LocalTime::rfc_3339())
        .init();

    debug!("Loading configuration (.env) ...");
    match dotenv() {
        Ok(path) => info!("Loaded: {}", path.display()),
        Err(_) => warn!("Failed to load .env file"),
    }

    let api_id = env::var_os("API_ID").and_then(|v| v.to_string_lossy().parse().ok()).unwrap_or(611335);
    let binding = env::var_os("API_HASH").unwrap_or_else(|| "d524b414d21f4d37f08684c1df41ac9c".into());
    let api_hash = binding.to_string_lossy();

    let session = Arc::new(SqliteSession::open(SESSION_FILE)?);
    let pool = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(&pool);
    let SenderPool {
        runner,
        updates,
        handle: _,
    } = pool;
    let _pool_task = tokio::spawn(runner.run());

    // TODO: (1) 发送消息

    if client.is_authorized().await? {
        info!("Client already authorized and ready to use!");
    } else {
        info!("Signing in...");
        let bot_token = env::var("BOT_TOKEN").context("BOT_TOKEN not set")?;
        match client.bot_sign_in(&bot_token, &api_hash).await {
            Ok(user) => info!("Account {} is logged in.", user.bare_id()),
            Err(err) => {
                error!("Failed to sign in as a bot :(\n{}", err);
                exit(Code::FAILURE.ok());
            }
        }
    }
    info!("Waiting for messages...");
    let mut updates = client.stream_updates(
        updates,
        UpdatesConfiguration {
            catch_up: true,
            ..Default::default()
        },
    );

    tokio::spawn(async move {
        loop {
            select! {
                _ = signal::ctrl_c() => {
                    exit(Code::SUCCESS.ok());
                }
            }
        }
    });

    let tracker = TaskTracker::new();
    let token = CancellationToken::new();
    (async || {
        loop {
            select! {
                update = updates.next() => {
                    match update {
                        Ok(Update::NewMessage(message)) if !message.outgoing() && message.text().trim() == "/restart" => {
                            message.reply("正在重启").await;
                            // TODO: (2) 重启自身
                            match std::env::current_exe() {
                                Ok(path) => {
                                    let args: Vec<String> = env::args().skip(1).collect();
                                    let mut cmd = Command::new(path);
                                    
                                    // TODO:HACK: 建立通道
                                    let decapsulation_key = DecapsulationKey::generate(&ML_KEM_1024).unwrap();
                                    let encapsulation_key = decapsulation_key.encapsulation_key().unwrap();
                                    let encapsulation_key_bytes = encapsulation_key.key_bytes().unwrap();
                                    let encapsulation_key_bytes = encapsulation_key_bytes.as_ref();
                                    let retrieved_encapsulation_key = EncapsulationKey::new(&ML_KEM_1024, encapsulation_key_bytes).unwrap();
                                    let (ciphertext, bob_secret) = retrieved_encapsulation_key.encapsulate().unwrap();
                                    let ciphertext_bytes = ciphertext.as_ref();
                                    let alice_secret = decapsulation_key.decapsulate(Ciphertext::from(ciphertext_bytes)).unwrap();
                                    
                                    cmd.args(&args);
                                    client.disconnect();
                                    match cmd.spawn() {
                                        Ok(child) => {
                                            info!("Spawned child for restart (pid = {})", child.id());
                                            return;
                                        }
                                        Err(_) => {
                                        }
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                        Ok(update) => {
                            let handle = client.clone();
                            tracker.spawn(handle_update(handle, update).with_cancellation_token_owned(token.clone()));
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    })().await;
    // TODO: (3) 监听消息
    tracker.close();
    token.cancel();
    tracker.wait().await;
    Ok(())
}
