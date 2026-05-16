use crate::{app::App, ui};
use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, panic, sync::mpsc, time::Duration};

use crate::app::Route;
use agentic_protocol::{ChatStreamEvent, SessionSummary, UiMessage};
use cli::client::{FetchError, Fetcher};

enum ChatEvent {
    Stream(ChatStreamEvent),
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
    let mut sessions_rx: Option<mpsc::Receiver<Result<Vec<SessionSummary>, FetchError>>> = None;
    let mut sessions_open_rx: Option<mpsc::Receiver<Result<(String, Vec<UiMessage>), FetchError>>> =
        None;

    loop {
        // Spawn sessions list load when navigating to Sessions for the first time.
        if matches!(app.route, Route::Sessions) && !app.sessions_loaded && sessions_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            sessions_rx = Some(rx);
            let f = fetcher.clone();
            runtime.spawn(async move {
                let _ = tx.send(f.sessions_list().await);
            });
        }

        // Spawn session open task when a session is selected.
        if let Some(ref id) = app.sessions_open_pending.clone() {
            if sessions_open_rx.is_none() {
                let (tx, rx) = mpsc::channel();
                sessions_open_rx = Some(rx);
                let f = fetcher.clone();
                let id = id.clone();
                runtime.spawn(async move {
                    let result = f.sessions_get(&id).await.map(|msgs| (id, msgs));
                    let _ = tx.send(result);
                });
            }
        }

        // Spawn streaming task when a pending prompt is waiting.
        if matches!(app.route, Route::Chat) && app.chat_stream().is_pending() {
            let session_id = app
                .active_session
                .as_ref()
                .map(|s| s.id.clone())
                .unwrap_or_default();
            let message = app
                .chat_stream()
                .pending_prompt()
                .unwrap_or_default()
                .to_owned();

            let (tx, rx) = mpsc::channel();
            chat_rx = Some(rx);
            app.start_chat_stream();

            let f = fetcher.clone();
            runtime.spawn(async move {
                match f.chat_stream(&session_id, &message).await {
                    Err(e) => {
                        let _ = tx.send(ChatEvent::Err(e.to_string()));
                    }
                    Ok(stream) => {
                        tokio::pin!(stream);
                        while let Some(item) = stream.next().await {
                            match item {
                                Ok(chunk) => {
                                    let is_done = matches!(chunk, ChatStreamEvent::MessageDone);
                                    if tx.send(ChatEvent::Stream(chunk)).is_err() {
                                        return;
                                    }
                                    if is_done {
                                        return;
                                    }
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

        // Drain sessions list result.
        if let Some(ref rx) = sessions_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(list) => {
                        app.sessions_list = list;
                        app.sessions_loaded = true;
                    }
                    Err(_) => {
                        // Leave sessions_loaded = false so retries work on next navigation.
                    }
                }
                sessions_rx = None;
            }
        }

        // Drain session open result.
        if let Some(ref rx) = sessions_open_rx {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok((id, messages)) => app.open_loaded_session(&id, messages),
                    Err(_) => {
                        app.sessions_open_pending = None;
                    }
                }
                sessions_open_rx = None;
            }
        }

        // Drain pending chat events before rendering.
        if let Some(ref rx) = chat_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ChatEvent::Stream(event) => {
                        let is_done = matches!(event, ChatStreamEvent::MessageDone);
                        app.apply_chat_stream_event(event);
                        app.scroll_chat_to_bottom();
                        if is_done {
                            app.finish_chat_stream();
                            app.scroll_chat_to_bottom();
                            chat_rx = None;
                            break;
                        }
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
                _ => {}
            }
        }

        if !matches!(app.route, Route::Chat) {
            chat_rx = None;
        }
        if !matches!(app.route, Route::Sessions) {
            sessions_rx = None;
            sessions_open_rx = None;
        }
    }
    Ok(())
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Mouse capture is intentionally OFF so users can select text natively
    // (drag-to-highlight + Cmd/Ctrl+C). Up/Down keys still scroll the chat.
    execute!(stdout, EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(())
}

pub fn install_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}
