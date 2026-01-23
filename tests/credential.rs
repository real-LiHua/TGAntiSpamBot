#![warn(clippy::pedantic)]
use keyring::Entry;
use std::env;
use std::process::Command;
use std::result::Result::Ok;

#[test]
fn main() {
    let entry = Entry::new("tgbot_test", "test").unwrap();
    if !env::var("CHILD").is_ok() {
        let _ = entry.set_secret(&[1, 1, 4, 5, 1, 4]);
        if let Ok(path) = env::current_exe() {
            let args: Vec<String> = env::args().skip(1).collect();
            let mut cmd = Command::new(path);
            let _ = cmd.args(&args).env("CHILD", "1").spawn().unwrap();
        }
    }
    assert_eq!(entry.get_secret().unwrap(), Vec::from([1, 1, 4, 5, 1, 4]));
    if env::var("CHILD").is_ok() {
        let _ = entry.delete_credential();
    }
}
