use crate::app::{App, ChatStream};
use agentic_protocol::{ToolState, UiMessage, UiPart, UiRole};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Position, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Padding, Paragraph, Widget, Wrap},
};

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::new()), area);

    let page = centered_rect(area, 92, area.height.saturating_sub(2).max(10));
    let [history, input, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(page);

    render_history(app, history, frame.buffer_mut());
    let cursor_pos = render_input(app, input, frame.buffer_mut());
    render_footer(app, footer, frame.buffer_mut());

    if let Some(pos) = cursor_pos {
        frame.set_cursor_position(pos);
    }
}

fn render_history(app: &mut App, area: Rect, buf: &mut Buffer) {
    let inner_width = area.width.saturating_sub(4) as usize;
    let inner_height = area.height.saturating_sub(2);
    let mut lines: Vec<Line> = Vec::new();

    if let Some(session) = &app.active_session {
        for message in &session.messages {
            push_message_lines(message, false, &mut lines);
        }
    }

    let is_streaming = match &app.chat_stream {
        ChatStream::Streaming {
            user_msg,
            assistant,
        } => {
            push_message_lines(&UiMessage::user_text(user_msg.clone()), false, &mut lines);
            push_message_lines(assistant, true, &mut lines);
            true
        }
        _ => false,
    };

    if lines.is_empty() {
        lines.push(Line::from(
            "Start a conversation from home or type a message here.".dim(),
        ));
    }

    let total_lines: u16 = lines
        .iter()
        .map(|l| {
            let w = l.width();
            if inner_width == 0 || w == 0 {
                1u16
            } else {
                w.div_ceil(inner_width) as u16
            }
        })
        .sum();
    let scroll = app
        .chat_scroll()
        .min(total_lines.saturating_sub(inner_height));
    app.set_chat_scroll(scroll);

    let scroll_hint = if total_lines > inner_height {
        format!(" ↑↓ {}/{} ", scroll + inner_height, total_lines)
    } else {
        String::new()
    };

    let border_style = if is_streaming {
        Style::new().cyan()
    } else {
        Style::new().dark_gray()
    };
    let session_title = app
        .active_session
        .as_ref()
        .map(|s| format!(" {} ", s.title.chars().take(40).collect::<String>()))
        .unwrap_or_else(|| " Chat ".to_owned());

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .scroll((scroll, 0))
        .block(
            Block::bordered()
                .title_top(Line::from(vec![
                    " ".into(),
                    session_title.bold().cyan(),
                    " ".into(),
                ]))
                .title_bottom(Line::from(scroll_hint.dim()).right_aligned())
                .border_type(BorderType::Rounded)
                .border_style(border_style)
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .render(area, buf);
}

fn push_message_lines(message: &UiMessage, streaming: bool, lines: &mut Vec<Line>) {
    match message.role {
        UiRole::User => lines.push(Line::from("You".green().bold())),
        UiRole::Assistant => lines.push(Line::from("Faaido".cyan().bold())),
    }

    for (index, part) in message.parts.iter().enumerate() {
        let is_last_part = index == message.parts.len().saturating_sub(1);
        push_part_lines(part, streaming && is_last_part, lines);
    }

    if streaming && message.parts.is_empty() {
        lines.push(Line::from("  ▌".cyan()));
    }

    lines.push(Line::from(""));
}

fn push_part_lines(part: &UiPart, streaming: bool, lines: &mut Vec<Line>) {
    match part {
        UiPart::Text { content } => {
            push_content_lines("  ", content, Style::new(), streaming, lines)
        }
        UiPart::Reasoning { content } => {
            push_labeled_content("thought >", content, Style::new().dim(), false, lines)
        }
        UiPart::Tool {
            id: _,
            name,
            state,
            input,
            output,
            error,
        } => {
            let status = tool_state_label(*state);
            let status_style = if *state == ToolState::Failed {
                Style::new().red()
            } else {
                Style::new().cyan()
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled("tool > ", Style::new().dim()),
                Span::raw(name.clone()),
                Span::raw(" "),
                Span::styled(status, status_style),
            ]));
            if let Some(value) = input {
                push_json_lines("input", value, lines);
            }
            if let Some(value) = output {
                push_json_lines("output", value, lines);
            }
            if let Some(message) = error {
                push_labeled_content("tool error >", message, Style::new().red(), false, lines);
            }
            if *state == ToolState::AwaitingApproval {
                push_labeled_content(
                    "approval >",
                    "Press Y to approve or N to reject",
                    Style::new().yellow(),
                    false,
                    lines,
                );
            }
        }
        UiPart::Error { message } => {
            push_labeled_content("error >", message, Style::new().red(), false, lines)
        }
    }
}

fn push_labeled_content(
    label: &'static str,
    content: &str,
    style: Style,
    streaming: bool,
    lines: &mut Vec<Line>,
) {
    let mut content_lines = content.lines().peekable();
    if content_lines.peek().is_none() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(label, style),
        ]));
        return;
    }

    for (index, content_line) in content.lines().enumerate() {
        let prefix = if index == 0 {
            format!("  {label} ")
        } else {
            "  ".to_string() + &" ".repeat(label.len() + 1)
        };
        let suffix = if streaming && index == content.lines().count().saturating_sub(1) {
            "▌"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(format!("{content_line}{suffix}"), style),
        ]));
    }
}

fn push_content_lines(
    prefix: &'static str,
    content: &str,
    style: Style,
    streaming: bool,
    lines: &mut Vec<Line>,
) {
    if content.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled("▌", style),
        ]));
        return;
    }

    let line_count = content.lines().count();
    for (index, content_line) in content.lines().enumerate() {
        let suffix = if streaming && index == line_count.saturating_sub(1) {
            "▌"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            Span::raw(prefix),
            Span::styled(format!("{content_line}{suffix}"), style),
        ]));
    }
}

fn push_json_lines(label: &'static str, value: &serde_json::Value, lines: &mut Vec<Line>) {
    let json = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    push_labeled_content(label, &json, Style::new().dim(), false, lines);
}

fn tool_state_label(state: ToolState) -> &'static str {
    match state {
        ToolState::Streaming => "streaming",
        ToolState::Calling => "calling",
        ToolState::AwaitingApproval => "awaiting approval",
        ToolState::Complete => "complete",
        ToolState::Failed => "failed",
    }
}

fn render_input(app: &App, area: Rect, buf: &mut Buffer) -> Option<Position> {
    let is_busy = matches!(
        app.chat_stream,
        ChatStream::Streaming { .. } | ChatStream::Pending(_)
    );
    let block = Block::bordered()
        .title_top(Line::from(vec![
            "  > ".green(),
            "Message ".bold(),
            "[ MODE: ".dim(),
            app.active_mode().label().bold().cyan(),
            " ] ".dim(),
        ]))
        .title_bottom(
            Line::from(format!(" {} ", app.text_area_key_bindings_hint()))
                .right_aligned()
                .dim(),
        )
        .border_type(BorderType::Rounded)
        .border_style(Style::new().cyan())
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(area);

    let text = if is_busy {
        Text::from(Line::from("waiting for response...".dim()))
    } else if app.input().is_empty() {
        Text::from(Line::from("> Type a message...".dim()))
    } else {
        let display = app
            .input()
            .split('\n')
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        Text::from(
            super::wrap_chars(&display, inner.width)
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>(),
        )
    };

    Paragraph::new(text).block(block).render(area, buf);

    if is_busy {
        return None;
    }
    let head = &app.input()[..app.input_caret()];
    let displayed_head: String = head
        .split('\n')
        .map(|line| format!("> {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let (cx, cy) = super::caret_xy(&displayed_head, inner.width);
    Some(Position::new(inner.x + cx, inner.y + cy))
}

fn render_footer(app: &App, area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(vec![
        " CHAT ".bold().on_dark_gray(),
        "  ".into(),
        "MODE ".bold().green(),
        app.active_mode().label().dim(),
        "  ".into(),
        "↑↓ ".bold().cyan(),
        "scroll ".dim(),
        "SHIFT+TAB ".bold().cyan(),
        "mode ".dim(),
        "/sessions ".bold().cyan(),
        "history ".dim(),
        "debug:tool-stream ".bold().cyan(),
        "demo ".dim(),
        "ESC ".bold().magenta(),
        "quit".dim(),
    ]))
    .alignment(Alignment::Center)
    .render(area, buf);
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
