use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Clone, Default, Eq, PartialEq)]
pub enum Route {
    #[default]
    Home,
    About,
    Settings,
    Missing(String),
}

impl Route {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::About => "about",
            Self::Settings => "settings",
            Self::Missing(_) => "missing",
        }
    }
}

#[derive(Clone, Copy)]
pub enum MessageRole {
    Assistant,
    User,
}

pub struct SessionMessage {
    role: MessageRole,
    content: String,
}

impl SessionMessage {
    pub fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    pub fn role(&self) -> MessageRole {
        self.role
    }

    pub fn content(&self) -> &str {
        &self.content
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
    route: Route,
    input: String,
    messages: Vec<SessionMessage>,
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

    pub fn messages(&self) -> &[SessionMessage] {
        &self.messages
    }

    pub fn text_area_key_bindings_hint(&self) -> String {
        let submit = key_binding_names(TextAreaAction::Submit).join("/");
        let newline = key_binding_names(TextAreaAction::InsertNewline).join("/");

        format!("{submit}: send  {newline}: newline")
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Esc => true,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => true,
            _ if self.handle_text_area_key(key) => false,
            KeyCode::Char(character) => {
                self.input.push(character);
                false
            }
            _ => false,
        }
    }

    fn handle_text_area_key(&mut self, key: KeyEvent) -> bool {
        let Some(binding) = TEXTAREA_KEY_BINDINGS
            .iter()
            .find(|binding| binding.matches(key))
        else {
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

        if !matches!(self.route, Route::Home) {
            self.route = Route::Home;
        }

        self.messages
            .push(SessionMessage::new(MessageRole::User, prompt));
        self.messages.push(SessionMessage::new(
            MessageRole::Assistant,
            "I can turn that into a process plan once workflow execution is wired in.",
        ));
        self.input.clear();
    }

    fn handle_route_command(&mut self, prompt: &str) -> bool {
        let Some(command) = prompt.strip_prefix('/') else {
            return false;
        };

        let normalized = command.trim_end_matches('/').to_ascii_lowercase();

        match normalized.as_str() {
            "home" => self.route = Route::Home,
            "about" => self.route = Route::About,
            "settings" => self.route = Route::Settings,
            _ => self.route = Route::Missing(format!("/{command}")),
        }

        true
    }
}

fn key_binding_names(action: TextAreaAction) -> Vec<String> {
    TEXTAREA_KEY_BINDINGS
        .iter()
        .filter(|binding| binding.action == action)
        .map(TextAreaKeyBinding::display_name)
        .collect()
}
