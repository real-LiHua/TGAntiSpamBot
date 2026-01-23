#![warn(clippy::pedantic)]
use keyring::{KeyringEntry, set_global_service_name};
use std::env;
use std::process::Command;
use std::result::Result::Ok;

#[tokio::test]
async fn test() {
    set_global_service_name("example");
    let entry = KeyringEntry::try_new("test").unwrap();
    let magic = "114514";
    if !env::var("CHILD").is_ok() {
        entry.set_secret(magic).await.unwrap();
        if let Ok(path) = env::current_exe() {
            let args: Vec<String> = env::args().skip(1).collect();
            let mut cmd = Command::new(path);
            let _ = cmd.args(&args).env("CHILD", "1").spawn().unwrap();
        }
    }
    assert_eq!(entry.get_secret().await.unwrap(), "114514");
    if env::var("CHILD").is_ok() {
        entry.delete_secret().await.unwrap();
    }
}

#[tokio::main]
async fn main() {
    test();
}
