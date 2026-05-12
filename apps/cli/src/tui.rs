use crate::{app::App, ui};
use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyEventKind, MouseEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, panic, sync::mpsc, time::Duration};

use crate::app::Route;
use cli::client::Fetcher;

enum ChatEvent {
    Chunk(String),
    Done,
    Err(String),
}

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
    let mut chat_rx: Option<mpsc::Receiver<ChatEvent>> = None;

    loop {
        // Spawn streaming task when a pending prompt is waiting.
        if matches!(app.route, Route::Chat) && app.chat_stream().is_pending() {
            // Build the full message list: session history + new user message
            let messages = app
                .active_session
                .as_ref()
                .map(|s| s.to_chat_messages(app.chat_stream().pending_prompt()))
                .unwrap_or_default();

            let (tx, rx) = mpsc::channel();
            chat_rx = Some(rx);
            app.start_chat_stream();

            let f = fetcher.clone();
            runtime.spawn(async move {
                match f.chat_stream(&messages).await {
                    Err(e) => { let _ = tx.send(ChatEvent::Err(e.to_string())); }
                    Ok(stream) => {
                        tokio::pin!(stream);
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(chunk) => {
                                    if tx.send(ChatEvent::Chunk(chunk.text)).is_err() { return; }
                                }
                                Err(e) => {
                                    let _ = tx.send(ChatEvent::Err(e.to_string()));
                                    return;
                                }
                            }
                        }
                        let _ = tx.send(ChatEvent::Done);
                    }
                }
            });
        }

        // Drain pending chat events before rendering.
        if let Some(ref rx) = chat_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ChatEvent::Chunk(text) => {
                        app.append_chat_chunk(&text);
                        app.scroll_chat_to_bottom();
                    }
                    ChatEvent::Done => {
                        app.finish_chat_stream();
                        app.scroll_chat_to_bottom();
                        chat_rx = None;
                        break;
                    }
                    ChatEvent::Err(msg) => {
                        app.set_chat_error(msg);
                        chat_rx = None;
                        break;
                    }
                }
            }
        }

        terminal.draw(|frame| ui::render(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press && app.handle_key(key) => {
                    break;
                }
                Event::Mouse(mouse) => match mouse.kind {
                    MouseEventKind::ScrollUp if matches!(app.route, Route::Chat) => app.scroll_chat_up(),
                    MouseEventKind::ScrollDown if matches!(app.route, Route::Chat) => app.scroll_chat_down(),
                    _ => {}
                },
                _ => {}
            }
        }

        if !matches!(app.route, Route::Chat) {
            chat_rx = None;
        }
    }
    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, event::EnableMouseCapture)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), event::DisableMouseCapture, LeaveAlternateScreen)?;
    Ok(())
}

pub fn install_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}
