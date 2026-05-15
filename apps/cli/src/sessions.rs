use agentic_protocol::{ChatMessage, UiMessage, UiPart, UiRole};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: u64,
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
            created_at: (ms / 1000) as u64,
            messages: vec![],
        }
    }

    /// Build text-only model history; reasoning, tools, and UI errors stay UI-only for now.
    pub fn to_chat_messages(&self, new_user_msg: Option<&str>) -> Vec<ChatMessage> {
        let mut msgs: Vec<ChatMessage> = self
            .messages
            .iter()
            .filter_map(|message| {
                let role = match message.role {
                    UiRole::User => "user",
                    UiRole::Assistant => "assistant",
                };
                let content = text_content(&message.parts);
                (!content.is_empty()).then(|| ChatMessage {
                    role: role.to_owned(),
                    content,
                })
            })
            .collect();
        if let Some(msg) = new_user_msg {
            msgs.push(ChatMessage {
                role: "user".to_owned(),
                content: msg.to_owned(),
            });
        }
        msgs
    }
}

fn sessions_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_owned());
    PathBuf::from(home).join(".faaido").join("sessions")
}

pub fn save(session: &Session) {
    let dir = sessions_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(json) = serde_json::to_vec_pretty(session) {
        let _ = std::fs::write(dir.join(format!("{}.json", session.id)), json);
    }
}

pub fn load_all() -> Vec<Session> {
    let dir = sessions_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };
    let mut sessions: Vec<Session> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|e| {
            let data = std::fs::read(e.path()).ok()?;
            serde_json::from_slice(&data).ok()
        })
        .collect();
    sessions.sort_by_key(|b| std::cmp::Reverse(b.created_at));
    sessions
}

fn text_content(parts: &[UiPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            UiPart::Text { content } => Some(content.as_str()),
            UiPart::Reasoning { .. } | UiPart::Tool { .. } | UiPart::Error { .. } => None,
        })
        .filter(|content| !content.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
