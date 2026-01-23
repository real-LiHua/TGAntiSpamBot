#![warn(clippy::pedantic)]
use keyring::{Entry, credential, default};
use std::env;
use std::process::Command;
use std::result::Result::Ok;

#[test]
fn main() {
    let credential_builder = default::default_credential_builder();
    let persistence = credential_builder.persistence();
    if matches!(persistence, credential::CredentialPersistence::UntilDelete) {
        println!("The default credential builder persists credentials on disk!");
    } else {
        println!("The default credential builder doesn't persist credentials on disk!");
    }
    let entry = Entry::new("tgbot_test", "test").unwrap();
    if !env::var("CHILD").is_ok() {
        let _ = entry.set_secret(&[1, 1, 4, 5, 1, 4]);
        if let Ok(path) = env::current_exe() {
            let args: Vec<String> = env::args().skip(1).collect();
            let mut cmd = Command::new(path);
            let child = cmd.args(&args).env("CHILD", "1").spawn().unwrap();
            println!("Spawned child for restart (pid = {})", child.id());
        }
    }
    assert_eq!(entry.get_secret().unwrap(), Vec::from([1, 1, 4, 5, 1, 4]));
}
