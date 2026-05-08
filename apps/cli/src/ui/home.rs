use crate::app::{App, MessageRole};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::Style,
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Widget, Wrap},
};

const APP_SUBTITLE: &str = agentic_core::DISPLAY_NAME;
const PROMPT_PLACEHOLDER: &str = "Ask Faaido anything about your process...";
const WORDMARK_LINES: [&str; 6] = [
    " ______           _     _        ",
    "|  ____|         (_)   | |       ",
    "| |__ __ _  __ _  _  __| | ___   ",
    "|  __/ _` |/ _` || |/ _` |/ _ \\  ",
    "| | | (_| | (_| || | (_| | (_) | ",
    "|_|  \\__,_|\\__,_||_|\\__,_|\\___/  ",
];

pub struct HomeScreen<'a> {
    app: &'a App,
}

impl<'a> HomeScreen<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for HomeScreen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("").style(Style::new()).render(area, buf);

        let home = centered_rect(area, 92, 26);
        let [inner] = Layout::vertical([Constraint::Fill(1)])
            .margin(1)
            .areas(home);

        render_home(inner, buf, self.app);
    }
}

fn render_home(area: Rect, buf: &mut Buffer, app: &App) {
    if app.messages().is_empty() {
        render_landing_home(area, buf, app);
        return;
    }

    let [brand, session, prompt, footer] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(area);

    CompactHeader.render(brand, buf);
    SessionTranscript::new(app).render(session, buf);
    HomeTextArea::new(app.input(), app.text_area_key_bindings_hint()).render(prompt, buf);
    StatusFooter::new(app.input_len()).render(footer, buf);
}

fn render_landing_home(area: Rect, buf: &mut Buffer, app: &App) {
    let landing = centered_rect(area, area.width.min(82), 17);
    let [brand, prompt, footer] = Layout::vertical([
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(landing);

    LandingHeader.render(brand, buf);
    HomeTextArea::new(app.input(), app.text_area_key_bindings_hint()).render(prompt, buf);
    StatusFooter::new(app.input_len()).render(footer, buf);
}

struct LandingHeader;

impl Widget for LandingHeader {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut lines = WORDMARK_LINES
            .iter()
            .map(|line| Line::from(line.bold().cyan()))
            .collect::<Vec<_>>();

        lines.push(Line::from(APP_SUBTITLE.dim()));
        lines.push(Line::from(
            "Ask Faaido anything about your process".dark_gray(),
        ));

        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}

struct CompactHeader;

impl Widget for CompactHeader {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = Line::from(vec![
            "Faaido".bold().cyan(),
            "  ".into(),
            APP_SUBTITLE.dim(),
        ]);

        Paragraph::new(text)
            .alignment(Alignment::Center)
            .render(area, buf);
    }
}

struct SessionTranscript<'a> {
    app: &'a App,
}

impl<'a> SessionTranscript<'a> {
    fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for SessionTranscript<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let content = centered_rect(area, area.width.min(76), area.height);
        let mut lines = Vec::new();

        for message in self.app.messages() {
            let author = match message.role() {
                MessageRole::Assistant => "Faaido".bold().cyan(),
                MessageRole::User => "You".bold().green(),
            };

            for (index, line) in message.content().lines().enumerate() {
                let content = match message.role() {
                    MessageRole::Assistant => line.dim(),
                    MessageRole::User => line.into(),
                };

                if index == 0 {
                    lines.push(Line::from(vec![author.clone(), "  ".into(), content]));
                } else {
                    lines.push(Line::from(vec!["        ".into(), content]));
                }
            }

            lines.push(Line::from(""));
        }

        let max_lines = content.height as usize;
        if lines.len() > max_lines {
            let hidden_lines = lines.len() - max_lines;
            lines = lines.into_iter().skip(hidden_lines).collect();
        }

        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: true })
            .render(content, buf);
    }
}

struct HomeTextArea<'a> {
    input: &'a str,
    key_bindings_hint: String,
}

impl<'a> HomeTextArea<'a> {
    fn new(input: &'a str, key_bindings_hint: String) -> Self {
        Self {
            input,
            key_bindings_hint,
        }
    }
}

impl Widget for HomeTextArea<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let text = if self.input.is_empty() {
            Text::from(Line::from(PROMPT_PLACEHOLDER.dim()))
        } else {
            Text::from(
                self.input
                    .split('\n')
                    .map(|line| Line::from(line.to_owned()).cyan())
                    .collect::<Vec<_>>(),
            )
        };

        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(
                Block::bordered()
                    .title_top(Line::from(vec!["  > ".green(), "Ask Faaido ".bold()]))
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

struct StatusFooter {
    input_len: usize,
}

impl StatusFooter {
    fn new(input_len: usize) -> Self {
        Self { input_len }
    }
}

impl Widget for StatusFooter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Line::from(vec![
            " HOME ".bold().on_dark_gray(),
            " home ".dim(),
            "  ".into(),
            "/about ".bold().cyan(),
            "/settings ".bold().cyan(),
            "  ".into(),
            "INPUT ".bold().green(),
            format!("{} chars", self.input_len).dim(),
            "  ".into(),
            "ENTER ".bold().cyan(),
            "send ".dim(),
            "CTRL+ENTER ".bold().green(),
            "newline ".dim(),
            "  ".into(),
            "ESC ".bold().magenta(),
            "quit".dim(),
        ]))
        .alignment(Alignment::Center)
        .render(area, buf);
    }
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
