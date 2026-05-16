use agentic_protocol::UiMessage;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<UiMessage>,
}

impl Session {
    pub fn new(first_prompt: &str) -> Self {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        Session {
            id: format!("{ms:016x}"),
            title: first_prompt.chars().take(60).collect(),
            messages: vec![],
        }
    }
}
