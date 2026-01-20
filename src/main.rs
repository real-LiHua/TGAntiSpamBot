use anyhow::{Context, Result};
use aws_lc_rs::kem::{DecapsulationKey, EncapsulationKey, ML_KEM_1024};
use dotenvy::dotenv;
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_client::types::update::Update;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use nix::unistd::getuid;
use proc_exit::{Code, exit};
use rand::distr::{Alphanumeric, SampleString};
use rand::Rng;
use std::env;
use std::io::{Read, stdin};
use std::process::Stdio;
use std::result::Result::Ok;
use std::sync::Arc;
use tokio::{select, signal};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::future::FutureExt;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, fmt::time::LocalTime};

use tg_anti_spam_bot::handle_update;

const SESSION_FILE: &str = "bot.session";

#[tokio::main]
async fn main() -> Result<()> {
    tokio::spawn(async move {
        signal::ctrl_c().await;
        exit(Code::SUCCESS.ok());
    });

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

    // TODO: (1) 建立通道
    let mut encapsulation_key_bytes = Vec::new();
    stdin().read_to_end(&mut encapsulation_key_bytes);
    let retrieved_encapsulation_key = EncapsulationKey::new(&ML_KEM_1024, &encapsulation_key_bytes).unwrap();
    let (ciphertext, _bob_secret) = retrieved_encapsulation_key.encapsulate().unwrap();
    let _ciphertext_bytes = ciphertext.as_ref();
    // TODO: 密文发送给父进程
    // TODO: 派生子密钥

    let session = Arc::new(SqliteSession::open(SESSION_FILE)?);
    let pool = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(&pool);
    let SenderPool {
        runner,
        updates,
        handle: _,
    } = pool;
    let _pool_task = tokio::spawn(runner.run());

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

    let tracker = TaskTracker::new();
    let token = CancellationToken::new();
    (async || {
        // TODO: 重启完成通知

        let mut need_restart = false;
        loop {
            select! {
                update = updates.next() => {
                    match update {
                        Ok(Update::NewMessage(message)) if !message.outgoing() && message.text().trim() == "/restart" => {
                            // TODO: 判断是否为 bot 所有者

                            if need_restart {
                                message.reply("别点了，在重启了").await;
                                continue
                            }

                            need_restart = true;
                            message.reply("正在重启").await;

                            // TODO: (2) 重启自身
                            match std::env::current_exe() {
                                Ok(path) => {
                                    let args: Vec<String> = env::args().skip(1).collect();
                                    let mut cmd = Command::new(path);

                                    // TODO:HACK: 建立通道
                                    // TODO:创建密钥对
                                    let decapsulation_key = DecapsulationKey::generate(&ML_KEM_1024).unwrap(); // 私钥
                                    let encapsulation_key = decapsulation_key.encapsulation_key().unwrap(); // 公钥
                                    let encapsulation_key_bytes = encapsulation_key.key_bytes().unwrap();
                                    // HACK: 改用tempfile
                                    let socket_file = Alphanumeric.sample_string(&mut rand::rng(), rand::rng().random_range(8..18));
                                    // let (tx, mut rx) = pipe::pipe().unwrap();
                                    cmd.args(&args).stdin(Stdio::piped()).env("TEMPDIR", format!("/run/user/{}", getuid().to_string())).env("SOCKET_FILE", socket_file);

                                    client.disconnect();
                                    let mut child = match cmd.spawn() {
                                        Ok(child) => {
                                            info!("Spawned child for restart (pid = {})", child.id().unwrap());
                                            child
                                        }
                                        Err(_) => {
                                            warn!("failed to spawn command");
                                            continue;
                                        }
                                    };
                                    let mut stdin = child
                                        .stdin
                                        .take()
                                        .expect("child did not have a handle to stdin");
                                    stdin.write_all(encapsulation_key_bytes.as_ref()).await;
                                    drop(stdin);
                                    
                                    // TODO: 接收密文

                                    //let _alice_secret = decapsulation_key.decapsulate(Ciphertext::from(ciphertext_bytes)).unwrap();

                                    // TODO: 派生子密钥
                                    
                                    continue;
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
