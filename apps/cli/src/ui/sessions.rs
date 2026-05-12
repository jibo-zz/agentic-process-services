use crate::app::App;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span},
    widgets::{Block, BorderType, Paragraph, Widget},
};

pub struct SessionsScreen<'a> {
    app: &'a App,
}

impl<'a> SessionsScreen<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for SessionsScreen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("").style(Style::new()).render(area, buf);

        let page = centered_rect(area, 92, area.height.saturating_sub(2).max(10));
        let [list_area, footer] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .spacing(1)
        .areas(page);

        render_list(self.app, list_area, buf);
        render_footer(footer, buf);
    }
}

fn render_list(app: &App, area: Rect, buf: &mut Buffer) {
    let inner_height = area.height.saturating_sub(2) as usize;
    let cursor = app.sessions_cursor;
    let sessions = &app.sessions_list;

    // Compute scroll offset to keep cursor visible
    let scroll = if cursor >= inner_height {
        cursor - inner_height + 1
    } else {
        0
    };

    let mut lines = vec![];

    if sessions.is_empty() {
        lines.push(Line::from("No sessions yet. Start a conversation from the home screen.".dim()));
    } else {
        for (i, session) in sessions.iter().enumerate().skip(scroll).take(inner_height) {
            let is_selected = i == cursor;

            let date = format_ts(session.created_at);
            let title = if session.title.len() > 55 {
                format!("{}...", &session.title[..55])
            } else {
                session.title.clone()
            };
            let turns = format!("{} turn{}", session.turns.len(), if session.turns.len() == 1 { "" } else { "s" });

            if is_selected {
                lines.push(Line::from(vec![
                    Span::styled("  ▶ ", Style::new().cyan().bold()),
                    Span::styled(title, Style::new().white().bold()),
                    Span::raw("  "),
                    Span::styled(turns, Style::new().dark_gray()),
                    Span::raw("  "),
                    Span::styled(date, Style::new().dim()),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(title, Style::new().dim()),
                    Span::raw("  "),
                    Span::styled(turns, Style::new().dark_gray()),
                    Span::raw("  "),
                    Span::styled(date, Style::new().dark_gray()),
                ]));
            }
        }
    }

    let title_line = Line::from(vec![
        " ".into(),
        "Sessions".bold().cyan(),
        format!("  {} total ", sessions.len()).dim(),
    ]);

    use ratatui::widgets::Padding;
    Paragraph::new(lines)
        .block(
            Block::bordered()
                .title_top(title_line)
                .border_type(BorderType::Rounded)
                .border_style(Style::new().dark_gray())
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .render(area, buf);
}

fn render_footer(area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(vec![
        " SESSIONS ".bold().on_dark_gray(),
        "  ".into(),
        "↑↓ ".bold().cyan(),
        "navigate ".dim(),
        "ENTER ".bold().cyan(),
        "open ".dim(),
        "ESC ".bold().magenta(),
        "back".dim(),
    ]))
    .alignment(Alignment::Center)
    .render(area, buf);
}

fn format_ts(ts: u64) -> String {
    // Simple formatting: show seconds-ago for recent, date for older
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let age = now.saturating_sub(ts);
    if age < 60 {
        "just now".to_owned()
    } else if age < 3600 {
        format!("{}m ago", age / 60)
    } else if age < 86400 {
        format!("{}h ago", age / 3600)
    } else {
        format!("{}d ago", age / 86400)
    }
}

fn centered_rect(area: Rect, max_width: u16, height: u16) -> Rect {
    let width = area.width.min(max_width).max(1);
    let height = height.max(1);
    let [vertical] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [center] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(vertical);
    center
}
