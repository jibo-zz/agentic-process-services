use crate::app::App;
use agentic_protocol::AgentMode;
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Position, Rect},
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

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::new()), area);

    let home = centered_rect(area, 92, 26);
    let [inner] = Layout::vertical([Constraint::Fill(1)])
        .margin(1)
        .areas(home);

    let landing = centered_rect(inner, inner.width.min(82), 17);
    let [brand, prompt, footer] = Layout::vertical([
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(landing);

    frame.render_widget(LandingHeader, brand);
    let input_inner = render_text_area(frame, app, prompt);
    frame.render_widget(
        StatusFooter::new(app.input_len(), app.active_mode()),
        footer,
    );

    let head = &app.input()[..app.input_caret()];
    let (cx, cy) = super::caret_xy(head, input_inner.width);
    frame.set_cursor_position(Position::new(input_inner.x + cx, input_inner.y + cy));
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

fn render_text_area(frame: &mut Frame, app: &App, area: Rect) -> Rect {
    let input = app.input();
    let text = if input.is_empty() {
        Text::from(Line::from(PROMPT_PLACEHOLDER.dim()))
    } else {
        Text::from(
            input
                .split('\n')
                .map(|line| Line::from(line.to_owned()).cyan())
                .collect::<Vec<_>>(),
        )
    };

    let block = Block::bordered()
        .title_top(Line::from(vec![
            "  > ".green(),
            "Ask Faaido ".bold(),
            "[ MODE: ".dim(),
            app.active_mode().label().bold().cyan(),
            " ] ".dim(),
        ]))
        .title_bottom(
            Line::from(format!(" {} ", app.text_area_key_bindings_hint()))
                .right_aligned()
                .dim(),
        )
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().cyan())
        .padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }).block(block),
        area,
    );
    inner
}

struct StatusFooter {
    input_len: usize,
    mode: AgentMode,
}

impl StatusFooter {
    fn new(input_len: usize, mode: AgentMode) -> Self {
        Self { input_len, mode }
    }
}

impl Widget for StatusFooter {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Line::from(vec![
            " HOME ".bold().on_dark_gray(),
            "  ".into(),
            "INPUT ".bold().green(),
            format!("{} chars", self.input_len).dim(),
            "  ".into(),
            "ENTER ".bold().cyan(),
            "send ".dim(),
            "CTRL+ENTER ".bold().green(),
            "newline ".dim(),
            "  ".into(),
            "SHIFT+TAB ".bold().cyan(),
            "mode ".dim(),
            "  ".into(),
            "MODE ".bold().green(),
            self.mode.label().dim(),
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
