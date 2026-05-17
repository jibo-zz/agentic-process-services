use crate::app::{App, ToolAvailability};
use agentic_protocol::{ToolDescriptor, ToolExecutionKind, ToolOutputPolicy, ToolRisk};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Flex, Layout, Position, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Padding, Paragraph, Widget, Wrap},
};

const TOOL_INPUT_PLACEHOLDER: &str = "Press N, then describe a tool to propose later...";

pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    frame.render_widget(Paragraph::new("").style(Style::new()), area);

    let page = centered_rect(area, 112, area.height.saturating_sub(2).max(18));
    let [main, input, footer] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(5),
        Constraint::Length(1),
    ])
    .spacing(1)
    .areas(page);
    let [list, detail] = Layout::horizontal([Constraint::Percentage(62), Constraint::Fill(1)])
        .spacing(1)
        .areas(main);

    render_tool_list(app, list, frame.buffer_mut());
    render_tool_detail(app, detail, frame.buffer_mut());
    let input_inner = render_tool_input(app, input, frame.buffer_mut());
    render_footer(app, footer, frame.buffer_mut());

    if app.tools_input_focused {
        let head = &app.tools_input.as_str()[..app.tools_input.caret_byte()];
        let (cx, cy) = super::caret_xy(head, input_inner.width);
        frame.set_cursor_position(Position::new(input_inner.x + cx, input_inner.y + cy));
    }
}

fn render_tool_list(app: &App, area: Rect, buf: &mut Buffer) {
    let inner_height = area.height.saturating_sub(3) as usize;
    let loading = !app.tools_loaded;
    let mut lines = vec![Line::from(vec![
        Span::styled("Name", Style::new().bold().green()),
        Span::raw("                 "),
        Span::styled("Kind", Style::new().bold().green()),
        Span::raw("        "),
        Span::styled("Risk", Style::new().bold().green()),
        Span::raw("              "),
        Span::styled("Approval", Style::new().bold().green()),
        Span::raw("  "),
        Span::styled("Status", Style::new().bold().green()),
    ])];

    if loading {
        lines.push(Line::from("Loading tools...".dim()));
    } else if app.tools.is_empty() {
        lines.push(Line::from("No tools reported by CLI or server.".dim()));
    } else {
        let scroll = if app.tools_cursor >= inner_height {
            app.tools_cursor - inner_height + 1
        } else {
            0
        };
        for (index, item) in app.tools.iter().enumerate().skip(scroll).take(inner_height) {
            let selected = index == app.tools_cursor;
            let marker = if selected { "▶" } else { " " };
            let style = if selected {
                Style::new().cyan().bold()
            } else {
                Style::new().dim()
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {marker} "), Style::new().cyan()),
                Span::styled(fixed(&item.descriptor.name, 20), style),
                Span::raw("  "),
                Span::styled(
                    fixed(execution_label(item.descriptor.execution), 11),
                    Style::new(),
                ),
                Span::raw("  "),
                Span::styled(
                    fixed(risk_label(item.descriptor.risk), 17),
                    risk_style(item.descriptor.risk),
                ),
                Span::raw("  "),
                Span::styled(
                    fixed(
                        if item.descriptor.approval_required {
                            "Yes"
                        } else {
                            "No"
                        },
                        8,
                    ),
                    approval_style(item.descriptor.approval_required),
                ),
                Span::raw("  "),
                Span::styled(
                    status_label(item.availability),
                    status_style(item.availability),
                ),
            ]));
        }
    }

    Paragraph::new(lines)
        .block(
            Block::bordered()
                .title_top(Line::from(vec![
                    " ".into(),
                    "Tools".bold().cyan(),
                    format!("  {} total ", app.tools.len()).dim(),
                ]))
                .border_type(BorderType::Rounded)
                .border_style(Style::new().dark_gray())
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .render(area, buf);
}

fn render_tool_detail(app: &App, area: Rect, buf: &mut Buffer) {
    let item = app.tools.get(app.tools_cursor);
    let mut lines = Vec::new();
    if let Some(item) = item {
        push_descriptor_lines(&item.descriptor, item.availability, &mut lines);
    } else if app.tools_loaded {
        lines.push(Line::from("No tool selected.".dim()));
    } else {
        lines.push(Line::from("Waiting for server registry...".dim()));
    }

    if let Some(notice) = &app.tools_notice {
        lines.push(Line::from(""));
        lines.push(Line::from(vec!["notice > ".yellow(), notice.clone().dim()]));
    }

    Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: true })
        .block(
            Block::bordered()
                .title_top(Line::from(vec![
                    " ".into(),
                    "Details".bold().cyan(),
                    " ".into(),
                ]))
                .border_type(BorderType::Rounded)
                .border_style(Style::new().dark_gray())
                .padding(Padding::new(1, 1, 0, 0)),
        )
        .render(area, buf);
}

fn push_descriptor_lines(
    tool: &ToolDescriptor,
    availability: ToolAvailability,
    lines: &mut Vec<Line>,
) {
    lines.push(Line::from(tool.name.clone().bold().cyan()));
    lines.push(Line::from(tool.description.clone().dim()));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        "kind > ".green(),
        execution_label(tool.execution).into(),
    ]));
    lines.push(Line::from(vec![
        "risk > ".green(),
        risk_label(tool.risk).into(),
    ]));
    lines.push(Line::from(vec![
        "approval > ".green(),
        if tool.approval_required {
            "required"
        } else {
            "automatic"
        }
        .into(),
    ]));
    lines.push(Line::from(vec![
        "output > ".green(),
        output_policy_label(tool.output_policy).into(),
    ]));
    lines.push(Line::from(vec![
        "status > ".green(),
        Span::styled(status_label(availability), status_style(availability)),
    ]));
}

fn render_tool_input(app: &App, area: Rect, buf: &mut Buffer) -> Rect {
    let text = if app.tools_input.is_empty() {
        Text::from(Line::from(TOOL_INPUT_PLACEHOLDER.dim()))
    } else {
        Text::from(Line::from(app.tools_input.as_str().to_owned().cyan()))
    };
    let border_style = if app.tools_input_focused {
        Style::new().cyan()
    } else {
        Style::new().dark_gray()
    };

    let block = Block::bordered()
        .title_top(Line::from(vec!["  + ".green(), "New Tool Request ".bold()]))
        .title_bottom(
            Line::from(" N: focus  Enter: queue  Esc: cancel/back ")
                .right_aligned()
                .dim(),
        )
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .padding(Padding::new(1, 1, 1, 0));
    let inner = block.inner(area);
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .block(block)
        .render(area, buf);
    inner
}

fn render_footer(app: &App, area: Rect, buf: &mut Buffer) {
    Paragraph::new(Line::from(vec![
        " TOOLS ".bold().on_dark_gray(),
        "  ".into(),
        "↑↓ ".bold().cyan(),
        "navigate ".dim(),
        "N ".bold().green(),
        "new tool ".dim(),
        "ESC ".bold().magenta(),
        if app.tools_input_focused {
            "cancel"
        } else {
            "back"
        }
        .dim(),
    ]))
    .alignment(Alignment::Center)
    .render(area, buf);
}

fn execution_label(execution: ToolExecutionKind) -> &'static str {
    match execution {
        ToolExecutionKind::ServerNative => "Server",
        ToolExecutionKind::LocalProxy => "LocalProxy",
    }
}

fn risk_label(risk: ToolRisk) -> &'static str {
    match risk {
        ToolRisk::ReadOnly => "ReadOnly",
        ToolRisk::WritesFiles => "WritesFiles",
        ToolRisk::DeletesFiles => "DeletesFiles",
        ToolRisk::DeletesDirectories => "DeletesDirs",
        ToolRisk::Network => "Network",
        ToolRisk::ExternalProcess => "ExternalProc",
    }
}

fn output_policy_label(policy: ToolOutputPolicy) -> &'static str {
    match policy {
        ToolOutputPolicy::FullToModelSummaryToUi => "full to model, summary to UI",
        ToolOutputPolicy::SummaryOnly => "summary only",
        ToolOutputPolicy::FullAllowed => "full allowed",
    }
}

fn status_label(status: ToolAvailability) -> &'static str {
    match status {
        ToolAvailability::Active => "Active",
        ToolAvailability::MissingLocally => "Missing local",
        ToolAvailability::MissingRemotely => "Missing server",
    }
}

fn status_style(status: ToolAvailability) -> Style {
    match status {
        ToolAvailability::Active => Style::new().green(),
        ToolAvailability::MissingLocally => Style::new().yellow(),
        ToolAvailability::MissingRemotely => Style::new().red(),
    }
}

fn risk_style(risk: ToolRisk) -> Style {
    match risk {
        ToolRisk::ReadOnly => Style::new().green(),
        ToolRisk::Network => Style::new().cyan(),
        ToolRisk::WritesFiles | ToolRisk::DeletesFiles | ToolRisk::DeletesDirectories => {
            Style::new().yellow()
        }
        ToolRisk::ExternalProcess => Style::new().red(),
    }
}

fn approval_style(required: bool) -> Style {
    if required {
        Style::new().yellow()
    } else {
        Style::new().green()
    }
}

fn fixed(value: &str, width: usize) -> String {
    let truncated = value.chars().take(width).collect::<String>();
    format!("{truncated:<width$}")
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
