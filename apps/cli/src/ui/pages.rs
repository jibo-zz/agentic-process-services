use crate::app::{App, ServerHealth};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Widget, Wrap},
};

const COMMAND_PLACEHOLDER: &str = "Type /home, /about, or /settings...";

pub struct AboutPage<'a> {
    app: &'a App,
}

impl<'a> AboutPage<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for AboutPage<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let health = match self.app.server_health() {
            ServerHealth::Unknown => "Server: checking...".to_owned(),
            ServerHealth::Up { service, status } => {
                format!("Server: UP - {service} reports {status}")
            }
            ServerHealth::Down(error) => format!("Server: DOWN - {error}"),
            ServerHealth::Error(error) => format!("Server: ERROR - {error}"),
        };

        PageShell::new(
            self.app,
            "About",
            "Faaido is an agentic process workspace.",
            &health,
        )
        .render(area, buf);
    }
}

pub struct SettingsPage<'a> {
    app: &'a App,
}

impl<'a> SettingsPage<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for SettingsPage<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        PageShell::new(
            self.app,
            "Settings",
            "Configuration controls will appear here.",
            "Shared config should stay in agentic-config when both apps need it.",
        )
        .render(area, buf);
    }
}

pub struct MissingPage<'a> {
    app: &'a App,
    path: &'a str,
}

impl<'a> MissingPage<'a> {
    pub fn new(app: &'a App, path: &'a str) -> Self {
        Self { app, path }
    }
}

impl Widget for MissingPage<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        PageShell::new(
            self.app,
            "Not Found",
            self.path,
            "That page does not exist. Try /home, /about, or /settings.",
        )
        .render(area, buf);
    }
}

struct PageShell<'a> {
    app: &'a App,
    title: &'a str,
    summary: &'a str,
    detail: &'a str,
}

impl<'a> PageShell<'a> {
    fn new(app: &'a App, title: &'a str, summary: &'a str, detail: &'a str) -> Self {
        Self {
            app,
            title,
            summary,
            detail,
        }
    }
}

impl Widget for PageShell<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("").style(Style::new()).render(area, buf);

        let page = centered_rect(area, 82, 20);
        let [content, input, footer] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .spacing(1)
        .areas(page);

        let text = Text::from(vec![
            Line::from(self.title.bold().cyan()),
            Line::from(""),
            Line::from(self.summary.dim()),
            Line::from(self.detail.dark_gray()),
            Line::from(""),
            Line::from(vec![
                "Commands".bold().green(),
                "  /home  /about  /settings".dim(),
            ]),
        ]);

        Paragraph::new(text)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .block(
                Block::bordered()
                    .title_top(Line::from(vec![
                        " ".into(),
                        self.app.route().label().bold(),
                        " ".into(),
                    ]))
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().dark_gray())
                    .padding(Padding::new(1, 1, 1, 0)),
            )
            .render(content, buf);

        CommandInput::new(self.app.input(), self.app.text_area_key_bindings_hint())
            .render(input, buf);
        render_footer(self.app, footer, buf);
    }
}

struct CommandInput<'a> {
    input: &'a str,
    key_bindings_hint: String,
}

impl<'a> CommandInput<'a> {
    fn new(input: &'a str, key_bindings_hint: String) -> Self {
        Self {
            input,
            key_bindings_hint,
        }
    }
}

impl Widget for CommandInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = if self.input.is_empty() {
            Text::from(Line::from(COMMAND_PLACEHOLDER.dim()))
        } else {
            Text::from(Line::from(self.input.to_owned().cyan()))
        };

        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title_top(Line::from(vec!["  / ".green(), "Command ".bold()]))
                    .title_bottom(
                        Line::from(format!(" {} ", self.key_bindings_hint))
                            .right_aligned()
                            .dim(),
                    )
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().cyan())
                    .padding(Padding::new(1, 1, 1, 0)),
            )
            .render(area, buf);
    }
}

fn render_footer(app: &App, area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(vec![
        " PAGE ".bold().on_dark_gray(),
        format!(" {} ", app.route().label()).dim(),
        "/home ".bold().cyan(),
        "/about ".bold().cyan(),
        "/settings ".bold().cyan(),
        "ESC ".bold().magenta(),
        "quit".dim(),
    ]))
    .alignment(Alignment::Center)
    .render(area, buf);
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.min(max_width).max(1);
    let height = area.height.min(max_height).max(1);

    let [vertical] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [center] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(vertical);

    center
}
