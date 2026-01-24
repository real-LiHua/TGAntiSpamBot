#![warn(clippy::pedantic)]
use anyhow::Result;
use dotenvy::dotenv;
use grammers_client::client::{Client, UpdatesConfiguration};
use grammers_client::types::update::Update;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use keyring::{KeyringEntry, set_global_service_name};
use proc_exit::{Code, exit};
use std::env;
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

    let entry_api_id = KeyringEntry::try_new("APP_ID").unwrap_or_else(|_| exit(Code::FAILURE.ok()));
    let entry_api_hash =
        KeyringEntry::try_new("APP_HASH").unwrap_or_else(|_| exit(Code::FAILURE.ok()));
    let entry_bot_token =
        KeyringEntry::try_new("BOT_TOKEN").unwrap_or_else(|_| exit(Code::FAILURE.ok()));
    let _entry_bot_owner_id =
        KeyringEntry::try_new("BOT_OWNER_ID").unwrap_or_else(|_| exit(Code::FAILURE.ok()));
    let entry_ready_child =
        KeyringEntry::try_new("READY_CHILD").unwrap_or_else(|_| exit(Code::FAILURE.ok()));
    let entry_ready_father =
        KeyringEntry::try_new("READY_FATHER").unwrap_or_else(|_| exit(Code::FAILURE.ok()));

    #[allow(clippy::unreadable_literal)]
    let api_id = match env::var_os("API_ID") {
        Some(value) => value.to_string_lossy().into_owned(),
        _ => entry_api_id
            .get_secret()
            .await
            .unwrap_or_else(|_| "611335".into()),
    }
    .parse::<i32>()?;

    let binding = match env::var_os("API_HASH") {
        Some(value) => value,
        _ => entry_api_hash
            .get_secret()
            .await
            .unwrap_or_else(|_| "d524b414d21f4d37f08684c1df41ac9c".into())
            .into(),
    };
    let api_hash = binding.to_string_lossy();

    let token = CancellationToken::new();
    let tracker = TaskTracker::new();

    let is_enabled = String::from("1");

    loop {
        let flag = entry_ready_father.get_secret().await;
        if flag.is_ok() && flag.unwrap_or_else(|_| String::new()) == is_enabled {
            let _ = entry_ready_father.set_secret("0").await;
            let _ = entry_ready_child.set_secret("1").await;
            break;
        }
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
        let bot_token = match env::var("BOT_TOKEN") {
            Ok(value) => value,
            _ => entry_bot_token.get_secret().await.unwrap_or_else(|_| {
                error!("BOT_TOKEN must be set");
                exit(Code::FAILURE.ok());
            }),
        };
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

                    if let Ok(path) = std::env::current_exe() {
                        let args: Vec<String> = env::args().skip(1).collect();
                        let mut cmd = Command::new(path);

                        cmd.args(&args);
                        client.disconnect();
                        let _ = entry_ready_father.set_secret("1").await;
                        if let Ok(child) = cmd.spawn() {
                            let pid = child.id().unwrap_or(0);
                            if pid != 0 {
                                info!("Spawned child for restart (pid = {})", pid);
                            } else {
                                warn!("Spawned child for restart (pid = -1)");
                            }
                        } else {
                            need_restart = false;
                            warn!("failed to spawn command");
                            let _ = message.reply("重启失败").await;
                            continue;
                        }
                    }

                    loop {
                        let flag = entry_ready_child.get_secret().await;
                        if flag.is_ok() && flag.unwrap_or_else(|_| String::new()) == is_enabled {
                            break;
                        }
                    }
                    break;
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

    tracker.close();
    token.cancel();
    tracker.wait().await;
    let _ = entry_ready_child.set_secret("0").await;
    Ok(())
}
