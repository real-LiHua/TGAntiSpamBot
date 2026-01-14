use grammers_client::Client;
use grammers_client::types::update::Update;

pub async fn handle_update(_client: Client, update: Update) {
    match update {
        Update::NewMessage(message) if !message.outgoing() => {
            let peer = message.peer().unwrap();
            println!(
                "Responding to {}",
                peer.name().unwrap_or(&format!("id {}", message.peer_id()))
            );
        }
        _ => {}
    }
}
