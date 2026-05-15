use crate::sessions::{self, Session};
use agentic_protocol::{ChatStreamEvent, ToolState, UiMessage, UiPart, UiRole};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Default, Eq, PartialEq)]
pub enum Route {
    #[default]
    Home,
    Chat,
    Sessions,
    Missing(String),
}

impl Route {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Chat => "chat",
            Self::Sessions => "sessions",
            Self::Missing(_) => "missing",
        }
    }
}

#[derive(Default)]
pub enum ChatStream {
    #[default]
    Idle,
    Pending(String), // user message, not yet sent
    Streaming {
        user_msg: String,
        assistant: UiMessage,
    }, // live turn
}

impl ChatStream {
    pub fn is_pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }

    pub fn pending_prompt(&self) -> Option<&str> {
        if let Self::Pending(p) = self {
            Some(p)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TextAreaAction {
    Submit,
    InsertNewline,
    Backspace,
}

struct TextAreaKeyBinding {
    name: &'static str,
    code: KeyCode,
    modifiers: KeyModifiers,
    action: TextAreaAction,
}

impl TextAreaKeyBinding {
    fn matches(&self, key: KeyEvent) -> bool {
        key.code == self.code && key.modifiers == self.modifiers
    }
    fn display_name(&self) -> String {
        if self.modifiers == KeyModifiers::CONTROL {
            format!("Ctrl+{}", self.name)
        } else if self.modifiers == KeyModifiers::ALT {
            format!("Alt+{}", self.name)
        } else {
            self.name.to_string()
        }
    }
}

const TEXTAREA_KEY_BINDINGS: &[TextAreaKeyBinding] = &[
    TextAreaKeyBinding {
        name: "return",
        code: KeyCode::Enter,
        modifiers: KeyModifiers::NONE,
        action: TextAreaAction::Submit,
    },
    TextAreaKeyBinding {
        name: "return",
        code: KeyCode::Enter,
        modifiers: KeyModifiers::CONTROL,
        action: TextAreaAction::InsertNewline,
    },
    TextAreaKeyBinding {
        name: "return",
        code: KeyCode::Enter,
        modifiers: KeyModifiers::ALT,
        action: TextAreaAction::InsertNewline,
    },
    TextAreaKeyBinding {
        name: "backspace",
        code: KeyCode::Backspace,
        modifiers: KeyModifiers::NONE,
        action: TextAreaAction::Backspace,
    },
];

#[derive(Default)]
pub struct App {
    pub route: Route,
    input: String,
    // chat
    pub active_session: Option<Session>,
    pub chat_stream: ChatStream,
    pub chat_scroll: u16,
    // sessions list
    pub sessions_list: Vec<Session>,
    pub sessions_cursor: usize,
    pub sessions_scroll: u16,
}

impl App {
    pub fn route(&self) -> &Route {
        &self.route
    }
    pub fn input(&self) -> &str {
        &self.input
    }
    pub fn input_len(&self) -> usize {
        self.input.len()
    }
    pub fn chat_stream(&self) -> &ChatStream {
        &self.chat_stream
    }
    pub fn chat_scroll(&self) -> u16 {
        self.chat_scroll
    }
    pub fn set_chat_scroll(&mut self, v: u16) {
        self.chat_scroll = v;
    }
    pub fn scroll_chat_up(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_sub(1);
    }
    pub fn scroll_chat_down(&mut self) {
        self.chat_scroll = self.chat_scroll.saturating_add(1);
    }
    pub fn scroll_chat_to_bottom(&mut self) {
        self.chat_scroll = u16::MAX;
    }

    /// Called by tui.rs when it picks up the Pending state and spawns the task.
    pub fn start_chat_stream(&mut self) {
        if let ChatStream::Pending(user_msg) = std::mem::take(&mut self.chat_stream) {
            self.chat_stream = ChatStream::Streaming {
                user_msg,
                assistant: UiMessage::assistant(),
            };
        }
    }

    pub fn apply_chat_stream_event(&mut self, event: ChatStreamEvent) {
        let ChatStream::Streaming { assistant, .. } = &mut self.chat_stream else {
            return;
        };

        match event {
            ChatStreamEvent::MessageStart { role } => {
                if role == UiRole::Assistant && assistant.role != UiRole::Assistant {
                    assistant.role = UiRole::Assistant;
                }
            }
            ChatStreamEvent::TextDelta { text } => append_text_part(assistant, text),
            ChatStreamEvent::ReasoningDelta { text } => append_reasoning_part(assistant, text),
            ChatStreamEvent::ToolUpdate {
                id,
                name,
                state,
                input,
                output,
                error,
            } => {
                upsert_tool_part(assistant, id, name, state, input, output, error);
            }
            ChatStreamEvent::Error { message } => assistant.parts.push(UiPart::Error { message }),
            ChatStreamEvent::MessageDone => {}
        }
    }

    /// Saves the completed turn into the session and persists to disk.
    pub fn finish_chat_stream(&mut self) {
        if let ChatStream::Streaming {
            user_msg,
            assistant,
        } = std::mem::take(&mut self.chat_stream)
            && let Some(session) = &mut self.active_session
        {
            session.messages.push(UiMessage::user_text(user_msg));
            session.messages.push(assistant);
            sessions::save(session);
        }
    }

    pub fn set_chat_error(&mut self, error: impl Into<String>) {
        let msg = error.into();
        if let Some(session) = &mut self.active_session {
            let (user_msg, mut assistant) = match std::mem::take(&mut self.chat_stream) {
                ChatStream::Streaming {
                    user_msg,
                    assistant,
                } => (user_msg, assistant),
                ChatStream::Pending(user_msg) => (user_msg, UiMessage::assistant()),
                ChatStream::Idle => (String::new(), UiMessage::assistant()),
            };
            assistant.parts.push(UiPart::Error { message: msg });
            if !user_msg.is_empty() {
                session.messages.push(UiMessage::user_text(user_msg));
            }
            session.messages.push(assistant);
            sessions::save(session);
        }
        self.chat_stream = ChatStream::Idle;
    }

    pub fn text_area_key_bindings_hint(&self) -> String {
        let submit = key_binding_names(TextAreaAction::Submit).join("/");
        let newline = key_binding_names(TextAreaAction::InsertNewline).join("/");
        format!("{submit}: send  {newline}: newline")
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            // Esc on sessions goes back to home; everywhere else it quits
            KeyCode::Esc if matches!(self.route, Route::Sessions) => {
                self.route = Route::Home;
                false
            }
            KeyCode::Esc => true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,

            // Sessions list navigation
            KeyCode::Up if matches!(self.route, Route::Sessions) => {
                self.sessions_cursor = self.sessions_cursor.saturating_sub(1);
                false
            }
            KeyCode::Down if matches!(self.route, Route::Sessions) => {
                let max = self.sessions_list.len().saturating_sub(1);
                self.sessions_cursor = (self.sessions_cursor + 1).min(max);
                false
            }
            KeyCode::Enter if matches!(self.route, Route::Sessions) => {
                self.open_selected_session();
                false
            }

            // Chat scroll
            KeyCode::Up if matches!(self.route, Route::Chat) => {
                self.scroll_chat_up();
                false
            }
            KeyCode::Down if matches!(self.route, Route::Chat) => {
                self.scroll_chat_down();
                false
            }

            // Text input (only on routes that have an input box)
            _ if matches!(self.route, Route::Sessions) => false,
            _ if self.handle_text_area_key(key) => false,
            KeyCode::Char(c) => {
                self.input.push(c);
                false
            }
            _ => false,
        }
    }

    fn handle_text_area_key(&mut self, key: KeyEvent) -> bool {
        let Some(binding) = TEXTAREA_KEY_BINDINGS.iter().find(|b| b.matches(key)) else {
            return false;
        };
        match binding.action {
            TextAreaAction::Submit => self.submit_prompt(),
            TextAreaAction::InsertNewline => self.input.push('\n'),
            TextAreaAction::Backspace => {
                self.input.pop();
            }
        }
        true
    }

    fn submit_prompt(&mut self) {
        let prompt = self.input.trim().to_owned();
        if prompt.is_empty() {
            return;
        }

        if self.handle_route_command(&prompt) {
            self.input.clear();
            return;
        }

        // Ignore submissions while a stream is active
        if matches!(
            self.chat_stream,
            ChatStream::Streaming { .. } | ChatStream::Pending(_)
        ) {
            self.input.clear();
            return;
        }

        if matches!(self.route, Route::Chat) {
            // Follow-up message in current session
            self.chat_stream = ChatStream::Pending(prompt);
            self.scroll_chat_to_bottom();
        } else {
            // New session from home (or wherever)
            let session = Session::new(&prompt);
            self.active_session = Some(session);
            self.chat_stream = ChatStream::Pending(prompt);
            self.route = Route::Chat;
            self.chat_scroll = 0;
            self.scroll_chat_to_bottom();
        }

        self.input.clear();
    }

    fn handle_route_command(&mut self, prompt: &str) -> bool {
        let Some(command) = prompt.strip_prefix('/') else {
            return false;
        };
        let normalized = command.trim_end_matches('/').to_ascii_lowercase();
        match normalized.as_str() {
            "home" => self.route = Route::Home,
            "sessions" => {
                self.sessions_list = sessions::load_all();
                self.sessions_cursor = 0;
                self.sessions_scroll = 0;
                self.route = Route::Sessions;
            }
            _ => self.route = Route::Missing(format!("/{command}")),
        }
        true
    }

    fn open_selected_session(&mut self) {
        let Some(session) = self.sessions_list.get(self.sessions_cursor).cloned() else {
            return;
        };
        self.active_session = Some(session);
        self.chat_stream = ChatStream::Idle;
        self.chat_scroll = u16::MAX;
        self.route = Route::Chat;
    }
}

fn append_text_part(message: &mut UiMessage, text: String) {
    match message.parts.last_mut() {
        Some(UiPart::Text { content }) => content.push_str(&text),
        _ => message.parts.push(UiPart::Text { content: text }),
    }
}

fn append_reasoning_part(message: &mut UiMessage, text: String) {
    match message.parts.last_mut() {
        Some(UiPart::Reasoning { content }) => content.push_str(&text),
        _ => message.parts.push(UiPart::Reasoning { content: text }),
    }
}

fn upsert_tool_part(
    message: &mut UiMessage,
    id: String,
    name: String,
    state: ToolState,
    input: Option<serde_json::Value>,
    output: Option<serde_json::Value>,
    error: Option<String>,
) {
    if let Some(UiPart::Tool {
        name: existing_name,
        state: existing_state,
        input: existing_input,
        output: existing_output,
        error: existing_error,
        ..
    }) = message
        .parts
        .iter_mut()
        .find(|part| matches!(part, UiPart::Tool { id: existing_id, .. } if existing_id == &id))
    {
        if !name.is_empty() {
            *existing_name = name;
        }
        *existing_state = state;
        if input.is_some() {
            *existing_input = input;
        }
        if output.is_some() {
            *existing_output = output;
        }
        if error.is_some() {
            *existing_error = error;
        }
        return;
    }

    message.parts.push(UiPart::Tool {
        id,
        name,
        state,
        input,
        output,
        error,
    });
}

fn key_binding_names(action: TextAreaAction) -> Vec<String> {
    TEXTAREA_KEY_BINDINGS
        .iter()
        .filter(|b| b.action == action)
        .map(TextAreaKeyBinding::display_name)
        .collect()
}
