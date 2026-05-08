use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Layout},
    style::Stylize,
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::{io, panic};

fn main() -> Result<()> {
    color_eyre::install()?;
    install_panic_hook();

    // Ratatui takes over the terminal while the app is running.
    let mut terminal = init_terminal()?;
    let result = run(&mut terminal);
    restore_terminal()?;

    result
}

fn run(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    // A simple event loop is enough for this welcome screen; async can be added
    // later if the CLI needs background work or server communication.
    loop {
        terminal.draw(render)?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
        {
            break;
        }
    }

    Ok(())
}

fn render(frame: &mut Frame) {
    let [_, content, _] = Layout::vertical([
        Constraint::Percentage(20),
        Constraint::Length(9),
        Constraint::Fill(1),
    ])
    .areas(frame.area());

    let [_, card, _] = Layout::horizontal([
        Constraint::Percentage(15),
        Constraint::Fill(1),
        Constraint::Percentage(15),
    ])
    .areas(content);

    let text = Text::from(vec![
        Line::from(agentic_core::DISPLAY_NAME.bold().cyan()),
        Line::from(""),
        Line::from("Cargo workspace ready".green()),
        Line::from("Axum server + Ratatui CLI".dim()),
        Line::from(""),
        Line::from(vec![
            "Press ".into(),
            "q".bold().magenta(),
            " or ".into(),
            "Esc".bold().magenta(),
            " to exit".into(),
        ]),
    ]);

    let widget = Paragraph::new(text).alignment(Alignment::Center).block(
        Block::new()
            .title(" Welcome ")
            .title_alignment(Alignment::Center)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(ratatui::style::Style::new().cyan()),
    );

    frame.render_widget(widget, card);
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);

    Ok(Terminal::new(backend)?)
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;

    Ok(())
}

fn install_panic_hook() {
    let original_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Always restore the terminal before printing panic details.
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}
