use crate::app::{App, ChatStream};
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Padding, Paragraph, Widget, Wrap},
};

pub struct ChatScreen<'a> {
    app: &'a mut App,
}

impl<'a> ChatScreen<'a> {
    pub fn new(app: &'a mut App) -> Self {
        Self { app }
    }
}

impl Widget for ChatScreen<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new("").style(Style::new()).render(area, buf);

        let page = centered_rect(area, 92, area.height.saturating_sub(2).max(10));
        let [history, input, footer] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .spacing(1)
        .areas(page);

        render_history(self.app, history, buf);
        render_input(self.app, input, buf);
        render_footer(footer, buf);
    }
}

fn render_history(app: &mut App, area: Rect, buf: &mut Buffer) {
    let inner_width = area.width.saturating_sub(4) as usize;
    let inner_height = area.height.saturating_sub(2);

    let mut lines: Vec<Line> = Vec::new();

    // Completed turns
    if let Some(session) = &app.active_session {
        for turn in &session.turns {
            // User turn
            lines.push(Line::from(Span::styled("You", Style::new().green().bold())));
            for content_line in turn.user.lines() {
                lines.push(Line::from(vec![Span::raw("  "), Span::raw(content_line.to_owned())]));
            }
            lines.push(Line::from(""));

            // Assistant turn
            lines.push(Line::from(Span::styled("Faaido", Style::new().cyan().bold())));
            for content_line in turn.assistant.lines() {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(content_line.to_owned(), Style::new().dim()),
                ]));
            }
            lines.push(Line::from(""));
        }
    }

    // In-progress turn
    let is_streaming = match &app.chat_stream {
        ChatStream::Streaming { user_msg, response } => {
            // Show user message
            lines.push(Line::from(Span::styled("You", Style::new().green().bold())));
            for content_line in user_msg.lines() {
                lines.push(Line::from(vec![Span::raw("  "), Span::raw(content_line.to_owned())]));
            }
            lines.push(Line::from(""));

            // Show streaming response
            lines.push(Line::from(Span::styled("Faaido", Style::new().cyan().bold())));
            if response.is_empty() {
                lines.push(Line::from("  ▌".cyan()));
            } else {
                for (i, content_line) in response.lines().enumerate() {
                    let is_last = i == response.lines().count().saturating_sub(1);
                    let text = if is_last { format!("  {content_line}▌") } else { format!("  {content_line}") };
                    lines.push(Line::from(text.cyan()));
                }
            }
            lines.push(Line::from(""));
            true
        }
        _ => false,
    };

    // Scroll clamping + writeback
    let total_lines: u16 = lines.iter().map(|l| {
        let w = l.width();
        if inner_width == 0 || w == 0 { 1u16 } else { w.div_ceil(inner_width) as u16 }
    }).sum();
    let scroll = app.chat_scroll().min(total_lines.saturating_sub(inner_height));
    app.set_chat_scroll(scroll);

    let scroll_hint = if total_lines > inner_height {
        format!(" ↑↓ {}/{} ", scroll + inner_height, total_lines)
    } else {
        String::new()
    };

    let border_style = if is_streaming { Style::new().cyan() } else { Style::new().dark_gray() };

    let session_title = app.active_session.as_ref()
        .map(|s| format!(" {} ", s.title.chars().take(40).collect::<String>()))
        .unwrap_or_else(|| " Chat ".to_owned());

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .scroll((scroll, 0))
        .block(
            Block::bordered()
                .title_top(Line::from(vec![" ".into(), session_title.bold().cyan(), " ".into()]))
                .title_bottom(Line::from(scroll_hint.dim()).right_aligned())
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .render(area, buf);
}

fn render_input(app: &App, area: Rect, buf: &mut Buffer) {
    let is_busy = matches!(app.chat_stream, ChatStream::Streaming { .. } | ChatStream::Pending(_));

    let (text, border_style) = if is_busy {
        (Text::from(Line::from("Waiting for response...".dim())), Style::new().dark_gray())
    } else if app.input().is_empty() {
        (Text::from(Line::from("Type a follow-up message...".dim())), Style::new().cyan())
    } else {
        (
            Text::from(app.input().split('\n').map(|l| Line::from(l.to_owned()).cyan()).collect::<Vec<_>>()),
            Style::new().cyan(),
        )
    };

    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(
            Block::bordered()
                .title_top(Line::from(vec!["  > ".green(), "Message ".bold()]))
                .title_bottom(
                    Line::from(format!(" {} ", app.text_area_key_bindings_hint())).right_aligned().dim(),
                )
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .padding(Padding::new(1, 1, 1, 0)),
        )
        .render(area, buf);
}

fn render_footer(area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(vec![
        " CHAT ".bold().on_dark_gray(),
        "  ".into(),
        "↑↓ ".bold().cyan(),
        "scroll ".dim(),
        "/sessions ".bold().cyan(),
        "history ".dim(),
        "ESC ".bold().magenta(),
        "quit".dim(),
    ]))
    .alignment(Alignment::Center)
    .render(area, buf);
}

fn centered_rect(area: Rect, max_width: u16, height: u16) -> Rect {
    let width = area.width.min(max_width).max(1);
    let height = height.max(1);
    let [vertical] = Layout::vertical([Constraint::Length(height)]).flex(Flex::Center).areas(area);
    let [center] = Layout::horizontal([Constraint::Length(width)]).flex(Flex::Center).areas(vertical);
    center
}
