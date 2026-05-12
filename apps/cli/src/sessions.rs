use agentic_protocol::ChatMessage;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub user: String,
    pub assistant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub created_at: u64,
    pub turns: Vec<Turn>,
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
            turns: vec![],
        }
    }

    /// Build the flat message list for the API (all completed turns + optional new user msg).
    pub fn to_chat_messages(&self, new_user_msg: Option<&str>) -> Vec<ChatMessage> {
        let mut msgs: Vec<ChatMessage> = self
            .turns
            .iter()
            .flat_map(|t| {
                [
                    ChatMessage { role: "user".to_owned(), content: t.user.clone() },
                    ChatMessage { role: "assistant".to_owned(), content: t.assistant.clone() },
                ]
            })
            .collect();
        if let Some(msg) = new_user_msg {
            msgs.push(ChatMessage { role: "user".to_owned(), content: msg.to_owned() });
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
