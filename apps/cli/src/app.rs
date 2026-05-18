use crate::sessions::Session;
use agentic_protocol::{
    AgentMode, ChatStreamEvent, LocalToolScript, SessionSummary, ToolDescriptor, ToolExecutionKind,
    ToolRow, ToolScriptLanguage, ToolState, ToolVersionStatus, UiMessage, UiPart, UiRole,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Debug)]
pub struct PendingToolApproval {
    pub invocation_id: String,
    pub name: String,
    pub input: serde_json::Value,
    /// For Tier-2 (DB-authored) tools, the script body to run on approval. None for Tier-1.
    pub script: Option<LocalToolScript>,
}

/// Single-buffer text input with a UTF-8 safe caret. Used for both the
/// chat/home prompt and the tools proposal input.
#[derive(Default, Clone)]
pub struct TextField {
    value: String,
    caret: usize,
}

impl TextField {
    pub fn from_initial(value: impl Into<String>) -> Self {
        let value = value.into();
        let caret = value.len();
        Self { value, caret }
    }
    pub fn as_str(&self) -> &str {
        &self.value
    }
    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }
    pub fn byte_len(&self) -> usize {
        self.value.len()
    }
    pub fn caret_byte(&self) -> usize {
        self.caret
    }
    pub fn clear(&mut self) {
        self.value.clear();
        self.caret = 0;
    }
    pub fn insert_char(&mut self, c: char) {
        self.value.insert(self.caret, c);
        self.caret += c.len_utf8();
    }
    pub fn backspace(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.value.replace_range(prev..self.caret, "");
            self.caret = prev;
        }
    }
    pub fn delete_forward(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.value.replace_range(self.caret..next, "");
        }
    }
    pub fn move_left(&mut self) {
        if let Some(prev) = self.prev_boundary() {
            self.caret = prev;
        }
    }
    pub fn move_right(&mut self) {
        if let Some(next) = self.next_boundary() {
            self.caret = next;
        }
    }
    pub fn move_home(&mut self) {
        let head = &self.value[..self.caret];
        self.caret = head.rfind('\n').map_or(0, |i| i + 1);
    }
    pub fn move_end(&mut self) {
        let tail = &self.value[self.caret..];
        let off = tail.find('\n').unwrap_or(tail.len());
        self.caret += off;
    }
    fn prev_boundary(&self) -> Option<usize> {
        if self.caret == 0 {
            return None;
        }
        let mut i = self.caret - 1;
        while !self.value.is_char_boundary(i) {
            i -= 1;
        }
        Some(i)
    }
    fn next_boundary(&self) -> Option<usize> {
        if self.caret >= self.value.len() {
            return None;
        }
        let mut i = self.caret + 1;
        while i < self.value.len() && !self.value.is_char_boundary(i) {
            i += 1;
        }
        Some(i)
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub enum Route {
    #[default]
    Home,
    Chat,
    Sessions,
    Tools,
    Missing(String),
}

impl Route {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Chat => "chat",
            Self::Sessions => "sessions",
            Self::Tools => "tools",
            Self::Missing(_) => "missing",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolAvailability {
    Active,
    Draft,
    MissingLocally,
    MissingRemotely,
}

#[derive(Clone, Debug)]
pub struct ToolDashboardItem {
    pub descriptor: ToolDescriptor,
    pub availability: ToolAvailability,
    /// Set for DB-backed tools (active or draft). `None` for built-in Tier-1.
    pub tool_id: Option<String>,
    /// Set only for drafts so `Enter` can reopen exactly that version.
    pub version_id: Option<String>,
    /// Set only for drafts; the script body and language used to reopen the editor.
    pub draft_script: Option<String>,
    pub draft_language: Option<ToolScriptLanguage>,
}

#[derive(Clone, Debug)]
pub struct PendingToolDelete {
    pub tool_id: String,
    pub name: String,
    pub is_draft: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolEditorField {
    Description,
    InputHint,
    OutputHint,
    Name,
    Script,
    Args,
}

#[derive(Clone, Debug, Default)]
pub enum GenerationState {
    #[default]
    Idle,
    Generating,
    Generated,
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct ToolEditorResult {
    pub kind: ToolEditorResultKind,
    pub message: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ToolEditorResultKind {
    Success,
    Failure,
}

#[derive(Clone, Default)]
pub struct ToolEditor {
    pub open: bool,
    pub field: Option<ToolEditorField>,
    pub description: TextField,
    pub input_hint: TextField,
    pub output_hint: TextField,
    pub name: TextField,
    pub language: ToolEditorLanguage,
    pub script: TextField,
    pub args: TextField,
    pub last_draft_version_id: Option<String>,
    pub last_result: Option<ToolEditorResult>,
    pub generation: GenerationState,
    pub generation_log: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ToolEditorLanguage {
    #[default]
    Python,
    Shell,
}

impl ToolEditorLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Shell => "shell",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Python => Self::Shell,
            Self::Shell => Self::Python,
        }
    }

    pub fn to_protocol(self) -> ToolScriptLanguage {
        match self {
            Self::Python => ToolScriptLanguage::Python,
            Self::Shell => ToolScriptLanguage::Shell,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ToolEditorAction {
    Generate,
    Run,
    SaveDraft,
    Register,
}

#[derive(Clone, Debug)]
pub struct ToolEditorSnapshot {
    pub name: String,
    pub language: ToolScriptLanguage,
    pub script: String,
    pub args: String,
    pub last_draft_version_id: Option<String>,
    pub description: String,
    pub input_hint: String,
    pub output_hint: String,
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
    input: TextField,
    pub active_mode: AgentMode,
    // chat
    pub active_session: Option<Session>,
    pub chat_stream: ChatStream,
    pub stream_mode: Option<AgentMode>,
    pub chat_scroll: u16,
    pub stream_id: Option<String>,
    pub stream_secret: Option<String>,
    pub pending_tool_approval: Option<PendingToolApproval>,
    approval_decision: Option<(PendingToolApproval, bool)>,
    // sessions list
    pub sessions_list: Vec<SessionSummary>,
    pub sessions_loaded: bool,
    pub sessions_cursor: usize,
    pub sessions_scroll: u16,
    pub sessions_open_pending: Option<String>,
    // tools dashboard
    pub tools: Vec<ToolDashboardItem>,
    pub tools_loaded: bool,
    pub tools_cursor: usize,
    pub tools_notice: Option<String>,
    pub tool_editor: ToolEditor,
    pending_editor_action: Option<ToolEditorAction>,
    pub pending_tool_delete: Option<PendingToolDelete>,
    tool_delete_decision: Option<(PendingToolDelete, bool)>,
    pending_reopen_draft: Option<ToolDashboardItem>,
}

impl App {
    pub fn route(&self) -> &Route {
        &self.route
    }
    pub fn input(&self) -> &str {
        self.input.as_str()
    }
    pub fn input_len(&self) -> usize {
        self.input.byte_len()
    }
    pub fn input_caret(&self) -> usize {
        self.input.caret_byte()
    }
    pub fn active_mode(&self) -> AgentMode {
        self.active_mode
    }
    pub fn stream_mode(&self) -> Option<AgentMode> {
        self.stream_mode
    }
    pub fn toggle_mode(&mut self) {
        self.active_mode = self.active_mode.next();
    }
    pub fn chat_stream(&self) -> &ChatStream {
        &self.chat_stream
    }
    fn chat_is_busy(&self) -> bool {
        matches!(
            self.chat_stream,
            ChatStream::Streaming { .. } | ChatStream::Pending(_)
        )
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
            self.stream_id = None;
            self.stream_secret = None;
            self.stream_mode = Some(self.active_mode);
            self.pending_tool_approval = None;
            self.approval_decision = None;
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
            ChatStreamEvent::StreamReady {
                stream_id,
                stream_secret,
            } => {
                self.stream_id = Some(stream_id);
                self.stream_secret = Some(stream_secret);
            }
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
            ChatStreamEvent::LocalToolRequest {
                invocation_id,
                name,
                input: _,
                approval_required,
                summary,
                script: _,
            } => {
                let state = if approval_required {
                    ToolState::AwaitingApproval
                } else {
                    ToolState::Calling
                };
                upsert_tool_part(
                    assistant,
                    invocation_id,
                    name,
                    state,
                    Some(serde_json::json!({ "summary": summary })),
                    None,
                    None,
                );
            }
            ChatStreamEvent::Error { message } => assistant.parts.push(UiPart::Error { message }),
            ChatStreamEvent::AuthorDone { .. } | ChatStreamEvent::MessageDone => {}
        }
    }

    /// Saves the completed turn into the in-memory session (server already persisted to DB).
    pub fn finish_chat_stream(&mut self) {
        if let ChatStream::Streaming {
            user_msg,
            assistant,
        } = std::mem::take(&mut self.chat_stream)
            && let Some(session) = &mut self.active_session
        {
            session.messages.push(UiMessage::user_text(user_msg));
            session.messages.push(assistant);
        }
        self.stream_id = None;
        self.stream_secret = None;
        self.stream_mode = None;
        self.pending_tool_approval = None;
        self.approval_decision = None;
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
        }
        self.chat_stream = ChatStream::Idle;
        self.stream_mode = None;
    }

    pub fn set_pending_tool_approval(&mut self, approval: PendingToolApproval) {
        self.pending_tool_approval = Some(approval);
    }

    pub fn has_pending_tool_approval(&self) -> bool {
        self.pending_tool_approval.is_some()
    }

    pub fn take_tool_approval_decision(&mut self) -> Option<(PendingToolApproval, bool)> {
        self.approval_decision.take()
    }

    pub fn local_tool_finished(
        &mut self,
        invocation_id: String,
        name: String,
        output: Option<serde_json::Value>,
        error: Option<String>,
    ) {
        let state = if error.is_some() {
            ToolState::Failed
        } else {
            ToolState::Complete
        };
        let ChatStream::Streaming { assistant, .. } = &mut self.chat_stream else {
            return;
        };
        upsert_tool_part(assistant, invocation_id, name, state, None, output, error);
    }

    pub fn local_tool_started(&mut self, invocation_id: String, name: String) {
        let ChatStream::Streaming { assistant, .. } = &mut self.chat_stream else {
            return;
        };
        upsert_tool_part(
            assistant,
            invocation_id,
            name,
            ToolState::Calling,
            None,
            None,
            None,
        );
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
            KeyCode::Esc if self.pending_tool_delete.is_some() => {
                self.pending_tool_delete = None;
                false
            }
            KeyCode::Esc if matches!(self.route, Route::Tools) && self.tool_editor.open => {
                self.close_tool_editor();
                false
            }
            KeyCode::Esc if matches!(self.route, Route::Tools) => {
                self.route = Route::Home;
                false
            }
            KeyCode::Esc => true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,

            KeyCode::Char('y') | KeyCode::Char('Y') if self.pending_tool_delete.is_some() => {
                if let Some(target) = self.pending_tool_delete.take() {
                    self.tool_delete_decision = Some((target, true));
                }
                false
            }
            KeyCode::Char('n') | KeyCode::Char('N') if self.pending_tool_delete.is_some() => {
                if let Some(target) = self.pending_tool_delete.take() {
                    self.tool_delete_decision = Some((target, false));
                }
                false
            }

            KeyCode::Char('y') | KeyCode::Char('Y') if self.has_pending_tool_approval() => {
                if let Some(approval) = self.pending_tool_approval.take() {
                    let invocation_id = approval.invocation_id.clone();
                    let name = approval.name.clone();
                    self.local_tool_started(invocation_id, name);
                    self.approval_decision = Some((approval, true));
                }
                false
            }
            KeyCode::Char('n') | KeyCode::Char('N') if self.has_pending_tool_approval() => {
                if let Some(approval) = self.pending_tool_approval.take() {
                    let invocation_id = approval.invocation_id.clone();
                    let name = approval.name.clone();
                    self.local_tool_finished(
                        invocation_id,
                        name,
                        None,
                        Some("User rejected the file operation".to_owned()),
                    );
                    self.approval_decision = Some((approval, false));
                }
                false
            }

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

            KeyCode::BackTab
                if matches!(self.route, Route::Home | Route::Chat) && !self.chat_is_busy() =>
            {
                self.toggle_mode();
                false
            }

            // Tools dashboard navigation and editor
            _ if matches!(self.route, Route::Tools) && self.tool_editor.open => {
                self.handle_tool_editor_key(key);
                false
            }
            KeyCode::Up if matches!(self.route, Route::Tools) => {
                self.tools_cursor = self.tools_cursor.saturating_sub(1);
                false
            }
            KeyCode::Down if matches!(self.route, Route::Tools) => {
                let max = self.tools.len().saturating_sub(1);
                self.tools_cursor = (self.tools_cursor + 1).min(max);
                false
            }
            KeyCode::Char('n') | KeyCode::Char('N') if matches!(self.route, Route::Tools) => {
                self.open_tool_editor();
                false
            }
            KeyCode::Char('d') | KeyCode::Char('D') | KeyCode::Delete
                if matches!(self.route, Route::Tools) =>
            {
                self.request_tool_delete();
                false
            }
            KeyCode::Enter if matches!(self.route, Route::Tools) => {
                self.request_reopen_draft();
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
            _ if matches!(self.route, Route::Tools) => false,
            KeyCode::Left => {
                self.input.move_left();
                false
            }
            KeyCode::Right => {
                self.input.move_right();
                false
            }
            KeyCode::Home => {
                self.input.move_home();
                false
            }
            KeyCode::End => {
                self.input.move_end();
                false
            }
            KeyCode::Delete => {
                self.input.delete_forward();
                false
            }
            _ if self.handle_text_area_key(key) => false,
            KeyCode::Char(c) => {
                self.input.insert_char(c);
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
            TextAreaAction::InsertNewline => self.input.insert_char('\n'),
            TextAreaAction::Backspace => self.input.backspace(),
        }
        true
    }

    fn submit_prompt(&mut self) {
        let prompt = self.input.as_str().trim().to_owned();
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
                if !self.sessions_loaded {
                    // tui.rs will spawn the async load when it sees Route::Sessions
                }
                self.sessions_cursor = 0;
                self.sessions_scroll = 0;
                self.route = Route::Sessions;
            }
            "tools" => {
                self.tools_cursor = 0;
                self.close_tool_editor();
                self.route = Route::Tools;
            }
            _ => self.route = Route::Missing(format!("/{command}")),
        }
        true
    }

    pub fn set_tools_from_server(
        &mut self,
        server_tools: Vec<ToolDescriptor>,
        management: Vec<ToolRow>,
    ) {
        let local_tools = agentic_tools::descriptors();
        let server_by_name = server_tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect::<HashMap<_, _>>();
        let local_by_name = local_tools
            .into_iter()
            .map(|tool| (tool.name.clone(), tool))
            .collect::<HashMap<_, _>>();
        let management_by_name: HashMap<String, ToolRow> = management
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        let names = server_by_name
            .keys()
            .chain(local_by_name.keys())
            .cloned()
            .collect::<BTreeSet<_>>();

        let mut items: Vec<ToolDashboardItem> = names
            .into_iter()
            .filter_map(|name| {
                let server = server_by_name.get(&name);
                let local = local_by_name.get(&name);
                let tool_id = management_by_name.get(&name).map(|t| t.id.clone());
                match (server, local) {
                    (Some(server), Some(local)) => Some(ToolDashboardItem {
                        descriptor: server.clone(),
                        availability: if tool_contract_matches(server, local) {
                            ToolAvailability::Active
                        } else {
                            ToolAvailability::MissingLocally
                        },
                        tool_id,
                        version_id: None,
                        draft_script: None,
                        draft_language: None,
                    }),
                    (Some(server), None) => Some(ToolDashboardItem {
                        descriptor: server.clone(),
                        availability: if server.execution == ToolExecutionKind::ServerNative {
                            ToolAvailability::Active
                        } else {
                            ToolAvailability::MissingLocally
                        },
                        tool_id,
                        version_id: None,
                        draft_script: None,
                        draft_language: None,
                    }),
                    (None, Some(local)) => Some(ToolDashboardItem {
                        descriptor: local.clone(),
                        availability: ToolAvailability::MissingRemotely,
                        tool_id: None,
                        version_id: None,
                        draft_script: None,
                        draft_language: None,
                    }),
                    (None, None) => None,
                }
            })
            .collect();

        // Append draft rows (newest first by tool, latest version first within tool).
        for (_, tool) in management_by_name.iter() {
            for version in &tool.versions {
                if !matches!(version.status, ToolVersionStatus::Draft) {
                    continue;
                }
                items.push(ToolDashboardItem {
                    descriptor: ToolDescriptor {
                        name: tool.name.clone(),
                        description: version.description.clone(),
                        execution: ToolExecutionKind::LocalProxy,
                        approval_required: !matches!(
                            version.risk,
                            agentic_protocol::ToolRisk::ReadOnly
                        ),
                        risk: version.risk,
                        output_policy: agentic_protocol::ToolOutputPolicy::SummaryOnly,
                        parameters: version.args_schema.clone(),
                    },
                    availability: ToolAvailability::Draft,
                    tool_id: Some(tool.id.clone()),
                    version_id: Some(version.id.clone()),
                    draft_script: Some(version.script.clone()),
                    draft_language: Some(version.language),
                });
            }
        }

        self.tools = items;
        self.tools_cursor = self.tools_cursor.min(self.tools.len().saturating_sub(1));
        self.tools_loaded = true;
    }

    pub fn set_tools_error(&mut self, message: impl Into<String>) {
        self.tools = agentic_tools::descriptors()
            .into_iter()
            .map(|descriptor| ToolDashboardItem {
                descriptor,
                availability: ToolAvailability::MissingRemotely,
                tool_id: None,
                version_id: None,
                draft_script: None,
                draft_language: None,
            })
            .collect();
        self.tools_loaded = true;
        self.tools_notice = Some(message.into());
    }

    fn request_tool_delete(&mut self) {
        let Some(item) = self.tools.get(self.tools_cursor) else {
            return;
        };
        let Some(tool_id) = item.tool_id.clone() else {
            self.tools_notice =
                Some("Built-in tools can't be deleted (compile-time only).".to_owned());
            return;
        };
        let is_draft = matches!(item.availability, ToolAvailability::Draft);
        self.pending_tool_delete = Some(PendingToolDelete {
            tool_id,
            name: item.descriptor.name.clone(),
            is_draft,
        });
    }

    pub fn take_tool_delete_decision(&mut self) -> Option<(PendingToolDelete, bool)> {
        self.tool_delete_decision.take()
    }

    fn request_reopen_draft(&mut self) {
        let Some(item) = self.tools.get(self.tools_cursor) else {
            return;
        };
        if !matches!(item.availability, ToolAvailability::Draft) {
            return;
        }
        self.pending_reopen_draft = Some(item.clone());
    }

    pub fn take_pending_reopen_draft(&mut self) -> Option<ToolDashboardItem> {
        self.pending_reopen_draft.take()
    }

    /// Re-opens an existing draft in the editor, pre-populated for review/publish.
    pub fn reopen_draft(&mut self, item: ToolDashboardItem) {
        let (Some(version_id), Some(script), Some(language)) =
            (item.version_id, item.draft_script, item.draft_language)
        else {
            return;
        };
        self.tools_notice = None;
        self.tool_editor.open = true;
        self.tool_editor.description = TextField::default();
        self.tool_editor.input_hint = TextField::default();
        self.tool_editor.output_hint = TextField::default();
        self.tool_editor.name = TextField::from_initial(item.descriptor.name.clone());
        self.tool_editor.script = TextField::from_initial(script);
        self.tool_editor.args = TextField::from_initial("{}");
        self.tool_editor.language = match language {
            ToolScriptLanguage::Python => ToolEditorLanguage::Python,
            ToolScriptLanguage::Shell => ToolEditorLanguage::Shell,
        };
        self.tool_editor.last_draft_version_id = Some(version_id);
        self.tool_editor.last_result = None;
        self.tool_editor.generation = GenerationState::Generated;
        self.tool_editor.generation_log.clear();
        self.tool_editor.field = Some(ToolEditorField::Script);
    }

    fn open_tool_editor(&mut self) {
        self.tools_notice = None;
        self.tool_editor.open = true;
        self.tool_editor.field = Some(ToolEditorField::Description);
        if self.tool_editor.args.is_empty() {
            // sensible default so the user sees the shape of ARGS_JSON.
            self.tool_editor.args = TextField::from_initial("{}");
        }
    }

    pub fn close_tool_editor(&mut self) {
        self.tool_editor.open = false;
        self.tool_editor.field = None;
    }

    pub fn take_pending_editor_action(&mut self) -> Option<ToolEditorAction> {
        self.pending_editor_action.take()
    }

    pub fn set_editor_result(&mut self, result: ToolEditorResult) {
        self.tool_editor.last_result = Some(result);
    }

    pub fn set_last_draft_version_id(&mut self, version_id: String) {
        self.tool_editor.last_draft_version_id = Some(version_id);
    }

    pub fn set_generation_state(&mut self, state: GenerationState) {
        self.tool_editor.generation = state;
    }

    pub fn push_generation_log(&mut self, line: impl Into<String>) {
        let line = line.into();
        if !line.is_empty() {
            self.tool_editor.generation_log.push(line);
            // keep the log bounded for the results pane
            if self.tool_editor.generation_log.len() > 80 {
                let overflow = self.tool_editor.generation_log.len() - 80;
                self.tool_editor.generation_log.drain(..overflow);
            }
        }
    }

    pub fn clear_generation_log(&mut self) {
        self.tool_editor.generation_log.clear();
    }

    pub fn apply_author_done(
        &mut self,
        version_id: String,
        name: String,
        language: ToolScriptLanguage,
        script: String,
        args_schema: serde_json::Value,
    ) {
        self.tool_editor.name = TextField::from_initial(name);
        self.tool_editor.script = TextField::from_initial(script);
        self.tool_editor.language = match language {
            ToolScriptLanguage::Python => ToolEditorLanguage::Python,
            ToolScriptLanguage::Shell => ToolEditorLanguage::Shell,
        };
        let args_default = args_schema
            .get("example")
            .cloned()
            .or_else(|| Some(serde_json::json!({})))
            .map(|v| serde_json::to_string(&v).unwrap_or_else(|_| "{}".to_owned()))
            .unwrap_or_else(|| "{}".to_owned());
        self.tool_editor.args = TextField::from_initial(args_default);
        self.tool_editor.last_draft_version_id = Some(version_id);
        self.tool_editor.generation = GenerationState::Generated;
        self.tool_editor.field = Some(ToolEditorField::Script);
    }

    pub fn editor_snapshot(&self) -> ToolEditorSnapshot {
        ToolEditorSnapshot {
            name: self.tool_editor.name.as_str().trim().to_owned(),
            language: self.tool_editor.language.to_protocol(),
            script: self.tool_editor.script.as_str().to_owned(),
            args: self.tool_editor.args.as_str().to_owned(),
            last_draft_version_id: self.tool_editor.last_draft_version_id.clone(),
            description: self.tool_editor.description.as_str().trim().to_owned(),
            input_hint: self.tool_editor.input_hint.as_str().trim().to_owned(),
            output_hint: self.tool_editor.output_hint.as_str().trim().to_owned(),
        }
    }

    fn handle_tool_editor_key(&mut self, key: KeyEvent) {
        // Action shortcuts (Ctrl-keyed) work regardless of focused field.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('g') | KeyCode::Char('G') => {
                    self.pending_editor_action = Some(ToolEditorAction::Generate);
                    return;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.pending_editor_action = Some(ToolEditorAction::Run);
                    return;
                }
                KeyCode::Char('s') | KeyCode::Char('S') => {
                    self.pending_editor_action = Some(ToolEditorAction::SaveDraft);
                    return;
                }
                KeyCode::Char('p') | KeyCode::Char('P') => {
                    self.pending_editor_action = Some(ToolEditorAction::Register);
                    return;
                }
                _ => {}
            }
        }
        match key.code {
            KeyCode::Tab => self.editor_focus_next(),
            KeyCode::BackTab => self.editor_focus_prev(),
            KeyCode::F(2) => self.tool_editor.language = self.tool_editor.language.toggle(),
            KeyCode::Enter
                if matches!(
                    self.tool_editor.field,
                    Some(ToolEditorField::Script) | Some(ToolEditorField::Description)
                ) =>
            {
                self.editor_insert_newline();
            }
            _ => self.editor_field_key(key),
        }
    }

    fn editor_focus_next(&mut self) {
        let cycle = self.editor_field_cycle();
        let current = self.tool_editor.field;
        let next = current
            .and_then(|f| {
                cycle
                    .iter()
                    .position(|&c| c == f)
                    .map(|i| cycle[(i + 1) % cycle.len()])
            })
            .unwrap_or(cycle[0]);
        self.tool_editor.field = Some(next);
    }

    fn editor_focus_prev(&mut self) {
        let cycle = self.editor_field_cycle();
        let current = self.tool_editor.field;
        let prev = current
            .and_then(|f| {
                cycle.iter().position(|&c| c == f).map(|i| {
                    let len = cycle.len();
                    cycle[(i + len - 1) % len]
                })
            })
            .unwrap_or(cycle[cycle.len() - 1]);
        self.tool_editor.field = Some(prev);
    }

    fn editor_field_cycle(&self) -> &'static [ToolEditorField] {
        match self.tool_editor.generation {
            GenerationState::Idle | GenerationState::Failed(_) => &[
                ToolEditorField::Description,
                ToolEditorField::InputHint,
                ToolEditorField::OutputHint,
            ],
            GenerationState::Generating => &[ToolEditorField::Description],
            GenerationState::Generated => &[
                ToolEditorField::Name,
                ToolEditorField::Script,
                ToolEditorField::Args,
            ],
        }
    }

    fn editor_field_key(&mut self, key: KeyEvent) {
        let Some(field) = self.tool_editor.field else {
            return;
        };
        let target: &mut TextField = match field {
            ToolEditorField::Description => &mut self.tool_editor.description,
            ToolEditorField::InputHint => &mut self.tool_editor.input_hint,
            ToolEditorField::OutputHint => &mut self.tool_editor.output_hint,
            ToolEditorField::Name => &mut self.tool_editor.name,
            ToolEditorField::Script => &mut self.tool_editor.script,
            ToolEditorField::Args => &mut self.tool_editor.args,
        };
        match key.code {
            KeyCode::Backspace => target.backspace(),
            KeyCode::Delete => target.delete_forward(),
            KeyCode::Left => target.move_left(),
            KeyCode::Right => target.move_right(),
            KeyCode::Home => target.move_home(),
            KeyCode::End => target.move_end(),
            KeyCode::Char(c) => target.insert_char(c),
            _ => {}
        }
    }

    fn editor_insert_newline(&mut self) {
        let Some(field) = self.tool_editor.field else {
            return;
        };
        let target: &mut TextField = match field {
            ToolEditorField::Description => &mut self.tool_editor.description,
            ToolEditorField::Script => &mut self.tool_editor.script,
            _ => return,
        };
        target.insert_char('\n');
    }

    fn open_selected_session(&mut self) {
        let Some(summary) = self.sessions_list.get(self.sessions_cursor) else {
            return;
        };
        // Signal tui.rs to load session messages asynchronously
        self.sessions_open_pending = Some(summary.id.clone());
    }

    /// Called by tui.rs after loading session messages from server.
    pub fn open_loaded_session(&mut self, id: &str, messages: Vec<UiMessage>) {
        let title = self
            .sessions_list
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.title.clone())
            .unwrap_or_default();
        self.active_session = Some(Session {
            id: id.to_owned(),
            title,
            messages,
        });
        self.chat_stream = ChatStream::Idle;
        self.stream_id = None;
        self.stream_secret = None;
        self.stream_mode = None;
        self.pending_tool_approval = None;
        self.approval_decision = None;
        self.chat_scroll = u16::MAX;
        self.route = Route::Chat;
        self.sessions_open_pending = None;
    }
}

fn tool_contract_matches(server: &ToolDescriptor, local: &ToolDescriptor) -> bool {
    server.execution == local.execution
        && server.approval_required == local.approval_required
        && server.risk == local.risk
        && server.output_policy == local.output_policy
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
