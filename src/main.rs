use dotenvy::dotenv;
use grammers_client::client::Client;
use grammers_mtsender::SenderPool;
use grammers_session::storages::SqliteSession;
use proc_exit::{Code, exit};
use std::env;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use tracing_subscriber::{EnvFilter, fmt, fmt::time::LocalTime};

const SESSION_FILE: &str = "bot.session";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_timer(LocalTime::rfc_3339())
        .init();

    debug!("Loading configuration (.env) ...");
    match dotenv() {
        Ok(path) => info!("Loaded: {}", path.display()),
        Err(_) => warn!("Failed to load .env file"),
    }

    let bot_token = match env::var("BOT_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        _ => {
            error!("Required environment variable BOT_TOKEN is not set or is empty");
            exit(Code::FAILURE.ok());
        }
    };
    let api_id = env::var_os("API_ID")
        .unwrap_or_else(|| "611335".into())
        .to_string_lossy()
        .trim()
        .parse::<i32>()
        .unwrap_or(611335);
    let binding =
        env::var_os("API_HASH").unwrap_or_else(|| "d524b414d21f4d37f08684c1df41ac9c".into());
    let api_hash = binding.to_string_lossy();

    let session = Arc::new(SqliteSession::open(SESSION_FILE)?);
    let pool = SenderPool::new(Arc::clone(&session), api_id);
    let client = Client::new(&pool);
    let SenderPool {
        runner,
        updates,
        handle,
    } = pool;
    let pool_task = tokio::spawn(runner.run());

    if client.is_authorized().await? {
        info!("Client already authorized and ready to use!");
    } else {
        info!("Signing in...");
        match client.bot_sign_in(&bot_token, &api_hash).await {
            Ok(user) => info!("Account {} is logged in.", user.bare_id()),
            Err(err) => {
                error!("Failed to sign in as a bot :(\n{}", err);
                exit(Code::FAILURE.ok());
            }
        };
    }
    info!("Waiting for messages...");
    Ok(())
}
