#![warn(clippy::pedantic)]
use anyhow::Result;
use dotenvy::dotenv;
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_client::types::update::Update;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use keyring::{KeyringEntry, get_global_service_name, native::Entry, set_global_service_name};
use proc_exit::{Code, exit};
use std::env;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::result::Result::Ok;
use std::string::String;
use std::sync::Arc;
use tokio::process::Command;
use tokio::signal;
use tokio_util::{future::FutureExt, sync::CancellationToken, task::TaskTracker};
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, fmt::time::LocalTime};

use tg_anti_spam_bot::handle_update;

const SESSION_FILE: &str = "bot.session";

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<()> {
    tokio::spawn(async move {
        let _ = signal::ctrl_c().await;
        exit(Code::SUCCESS.ok());
    });

    set_global_service_name(env!("CARGO_BIN_NAME"));

    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(LocalTime::rfc_3339())
        .init();

    debug!("Loading configuration (.env) ...");
    if let Ok(path) = dotenv() {
        info!("Loaded: {}", path.display());
    } else {
        warn!("Failed to load .env file");
    }

    // HACK: 机密服务可能不稳定
    let entry_api_id = Entry::new(get_global_service_name(), "APP_ID").unwrap();
    let entry_api_hash = Entry::new(get_global_service_name(), "APP_HASH").unwrap();
    let entry_bot_token = Entry::new(get_global_service_name(), "BOT_TOKEN").unwrap();
    let _entry_bot_owner_id = KeyringEntry::try_new("BOT_OWNER_ID").unwrap();
    let entry_ready_child = KeyringEntry::try_new("READY_CHILD").unwrap();
    let entry_ready_father = KeyringEntry::try_new("READY_FATHER").unwrap();

    #[allow(clippy::unreadable_literal)]
    let api_id = env::var_os("API_ID")
        .map(|v| v.to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            String::from_utf8_lossy(&entry_api_id.get_secret().unwrap_or("611335".into()))
                .into_owned()
        })
        .parse::<i32>()?;

    let binding = env::var_os("API_HASH").unwrap_or_else(|| {
        OsString::from_vec(
            entry_api_hash
                .get_secret()
                .unwrap_or("d524b414d21f4d37f08684c1df41ac9c".into()),
        )
    });
    let api_hash = binding.to_string_lossy();

    let token = CancellationToken::new();
    let tracker = TaskTracker::new();

    if entry_ready_father.get_secret().await.is_ok() {
        let _ = entry_ready_father.delete_secret().await;
        let _ = entry_ready_child.set_secret("1").await;
    }

    let session = Arc::new(SqliteSession::open(SESSION_FILE)?);
    let pool = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(&pool);
    let SenderPool {
        runner,
        updates,
        handle: _,
    } = pool;
    let _ = tokio::spawn(runner.run());
    if client.is_authorized().await? {
        info!("Client already authorized and ready to use!");
    } else {
        info!("Signing in...");
        let bot_token = env::var("BOT_TOKEN").unwrap_or_else(|_| {
            String::try_from(entry_bot_token.get_secret().unwrap_or_else(|_| {
                error!("BOT_TOKEN must be set");
                exit(Code::FAILURE.ok());
            }))
            .expect("Failed to convert bot token")
        });
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

    Box::pin(async {
        let mut need_restart = false;
        // TODO: 重启完成通知

        loop {
            match updates.next().await {
                Ok(Update::NewMessage(message))
                    if !message.outgoing() && message.text().trim() == "/restart" =>
                {
                    // TODO: 判断是否为 bot 所有者

                    if need_restart {
                        let _ = message.reply("别点了，在重启了").await;
                        continue;
                    }

                    need_restart = true;
                    let _ = message.reply("正在重启").await;

                    // TODO: (2) 重启自身
                    if let Ok(path) = std::env::current_exe() {
                        let args: Vec<String> = env::args().skip(1).collect();
                        let mut cmd = Command::new(path);

                        cmd.args(&args);
                        client.disconnect();
                        let _ = entry_ready_father.set_secret("1").await;
                        if let Ok(child) = cmd.spawn() {
                            info!("Spawned child for restart (pid = {})", child.id().unwrap());
                        } else {
                            warn!("failed to spawn command");
                        }
                    }

                    if entry_ready_child.get_secret().await.is_ok() {
                        break;
                    }
                }
                Ok(update) => {
                    let handle = client.clone();
                    tracker.spawn(
                        handle_update(handle, update).with_cancellation_token_owned(token.clone()),
                    );
                }
                Err(_) => {}
            }
        }
    })
    .await;

    // TODO: (3) 监听消息
    tracker.close();
    token.cancel();
    tracker.wait().await;
    let _ = entry_ready_child.delete_secret().await;
    Ok(())
}
