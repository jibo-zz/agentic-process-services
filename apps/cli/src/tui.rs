use crate::{app::App, ui};
use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, panic};

use crate::app::Route;
use cli::client::{FetchError, Fetcher};

pub fn run() -> Result<()> {
    let mut terminal = init_terminal()?;
    let result = run_app(&mut terminal);
    restore_terminal()?;

    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::default();
    let fetcher = Fetcher::local();
    let runtime = tokio::runtime::Runtime::new()?;

    loop {
        terminal.draw(|frame| ui::render(frame, &app))?;

        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
            && app.handle_key(key)
        {
            break;
        }

        refresh_about_health(&mut app, &fetcher, &runtime);
    }

    Ok(())
}

fn refresh_about_health(app: &mut App, fetcher: &Fetcher, runtime: &tokio::runtime::Runtime) {
    if !matches!(app.route(), Route::About) || !app.server_health().is_unknown() {
        return;
    }

    match runtime.block_on(fetcher.check_health()) {
        Ok(health) => app.set_server_up(health.service, health.status),
        Err(FetchError::Http(error)) => app.set_server_down(error.to_string()),
        Err(error) => app.set_server_error(error.to_string()),
    }
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

pub fn install_panic_hook() {
    let original_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Always restore the terminal before printing panic details.
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}
