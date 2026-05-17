use crate::{
    app::{App, PendingToolApproval},
    ui,
};
use color_eyre::eyre::Result;
use crossterm::{
    event::{self, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures_util::StreamExt;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Write},
    panic,
    sync::mpsc,
    time::Duration,
};

// OSC 12 sets the terminal cursor color; OSC 112 resets to the user's
// default. The hex matches ratatui's cyan accent used on focused borders.
const CURSOR_COLOR_SET: &str = "\x1b]12;#00d7d7\x07";
const CURSOR_COLOR_RESET: &str = "\x1b]112\x07";

use crate::app::Route;
use agentic_protocol::{
    ChatStreamEvent, SessionSummary, ToolResultParams, ToolsListResponse, UiMessage,
};
use agentic_tools::WorkspaceGuard;
use cli::client::{FetchError, Fetcher};
use cli::local_tools;

enum ChatEvent {
    Stream(ChatStreamEvent),
    LocalToolFinished {
        invocation_id: String,
        name: String,
        output: Option<serde_json::Value>,
        error: Option<String>,
    },
    Done,
    Err(String),
}

type SessionsOpenResult = Result<(String, Vec<UiMessage>), FetchError>;
type ToolsListResult = Result<ToolsListResponse, FetchError>;

pub fn run() -> Result<()> {
    let mut terminal = init_terminal()?;
    let result = run_app(&mut terminal);
    restore_terminal()?;
    result
}

fn run_app(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let mut app = App::default();
    let fetcher = Fetcher::local();
    let workspace_guard = WorkspaceGuard::from_current_dir()
        .map_err(|error| color_eyre::eyre::eyre!(error.to_string()))?;
    let runtime = tokio::runtime::Runtime::new()?;
    let mut chat_rx: Option<mpsc::Receiver<ChatEvent>> = None;
    let mut chat_tx: Option<mpsc::Sender<ChatEvent>> = None;
    let mut sessions_rx: Option<mpsc::Receiver<Result<Vec<SessionSummary>, FetchError>>> = None;
    let mut sessions_open_rx: Option<mpsc::Receiver<SessionsOpenResult>> = None;
    let mut tools_rx: Option<mpsc::Receiver<ToolsListResult>> = None;

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

        // Spawn tools registry load when navigating to Tools for the first time.
        if matches!(app.route, Route::Tools) && !app.tools_loaded && tools_rx.is_none() {
            let (tx, rx) = mpsc::channel();
            tools_rx = Some(rx);
            let f = fetcher.clone();
            runtime.spawn(async move {
                let _ = tx.send(f.tools_list().await);
            });
        }

        // Spawn session open task when a session is selected.
        if let Some(ref id) = app.sessions_open_pending.clone()
            && sessions_open_rx.is_none()
        {
            let (tx, rx) = mpsc::channel();
            sessions_open_rx = Some(rx);
            let f = fetcher.clone();
            let id = id.clone();
            runtime.spawn(async move {
                let result = f.sessions_get(&id).await.map(|msgs| (id, msgs));
                let _ = tx.send(result);
            });
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
            chat_tx = Some(tx.clone());
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
        if let Some(ref rx) = sessions_rx
            && let Ok(result) = rx.try_recv()
        {
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

        // Drain session open result.
        if let Some(ref rx) = sessions_open_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok((id, messages)) => app.open_loaded_session(&id, messages),
                Err(_) => {
                    app.sessions_open_pending = None;
                }
            }
            sessions_open_rx = None;
        }

        // Drain tools registry result.
        if let Some(ref rx) = tools_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(response) => app.set_tools_from_server(response.tools),
                Err(error) => {
                    app.set_tools_error(format!("Server tool registry unavailable: {error}"))
                }
            }
            tools_rx = None;
        }

        // Drain pending chat events before rendering.
        if let Some(ref rx) = chat_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ChatEvent::Stream(event) => {
                        let is_done = matches!(event, ChatStreamEvent::MessageDone);
                        handle_chat_stream_event(
                            &mut app,
                            &runtime,
                            &fetcher,
                            &workspace_guard,
                            chat_tx.as_ref(),
                            event,
                        );
                        app.scroll_chat_to_bottom();
                        if is_done {
                            app.finish_chat_stream();
                            app.scroll_chat_to_bottom();
                            chat_rx = None;
                            chat_tx = None;
                            break;
                        }
                    }
                    ChatEvent::LocalToolFinished {
                        invocation_id,
                        name,
                        output,
                        error,
                    } => {
                        app.local_tool_finished(invocation_id, name, output, error);
                        app.scroll_chat_to_bottom();
                    }
                    ChatEvent::Done => {
                        app.finish_chat_stream();
                        app.scroll_chat_to_bottom();
                        chat_rx = None;
                        chat_tx = None;
                        break;
                    }
                    ChatEvent::Err(msg) => {
                        app.set_chat_error(msg);
                        chat_rx = None;
                        chat_tx = None;
                        break;
                    }
                }
            }
        }

        terminal.draw(|frame| ui::render(frame, &mut app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let should_quit = app.handle_key(key);
                    if let Some((approval, approved)) = app.take_tool_approval_decision() {
                        handle_tool_approval_decision(
                            &mut app,
                            &runtime,
                            &fetcher,
                            &workspace_guard,
                            chat_tx.as_ref(),
                            approval,
                            approved,
                        );
                    }
                    if should_quit {
                        break;
                    }
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
        if !matches!(app.route, Route::Tools) {
            tools_rx = None;
        }
    }
    Ok(())
}

fn handle_chat_stream_event(
    app: &mut App,
    runtime: &tokio::runtime::Runtime,
    fetcher: &Fetcher,
    workspace_guard: &WorkspaceGuard,
    chat_tx: Option<&mpsc::Sender<ChatEvent>>,
    event: ChatStreamEvent,
) {
    match event {
        ChatStreamEvent::LocalToolRequest {
            invocation_id,
            name,
            input,
            approval_required,
            summary: _,
        } => {
            if approval_required {
                let summary = agentic_tools::approval_summary(&name, &input, workspace_guard);
                app.apply_chat_stream_event(ChatStreamEvent::LocalToolRequest {
                    invocation_id: invocation_id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    approval_required,
                    summary,
                });
                app.set_pending_tool_approval(PendingToolApproval {
                    invocation_id,
                    name,
                    input,
                });
                return;
            }

            let Some((stream_id, stream_secret)) = stream_auth(app) else {
                app.local_tool_finished(
                    invocation_id,
                    name,
                    None,
                    Some("Tool request arrived before stream authentication".to_owned()),
                );
                return;
            };
            if let Some(tx) = chat_tx.cloned() {
                spawn_local_tool(
                    runtime,
                    fetcher.clone(),
                    workspace_guard.clone(),
                    tx,
                    stream_id,
                    stream_secret,
                    invocation_id,
                    name,
                    input,
                    false,
                );
            }
        }
        event => app.apply_chat_stream_event(event),
    }
}

fn handle_tool_approval_decision(
    app: &mut App,
    runtime: &tokio::runtime::Runtime,
    fetcher: &Fetcher,
    workspace_guard: &WorkspaceGuard,
    chat_tx: Option<&mpsc::Sender<ChatEvent>>,
    approval: PendingToolApproval,
    approved: bool,
) {
    let Some((stream_id, stream_secret)) = stream_auth(app) else {
        app.local_tool_finished(
            approval.invocation_id,
            approval.name,
            None,
            Some("Tool approval could not be sent before stream authentication".to_owned()),
        );
        return;
    };
    let Some(tx) = chat_tx.cloned() else {
        return;
    };
    if approved {
        spawn_local_tool(
            runtime,
            fetcher.clone(),
            workspace_guard.clone(),
            tx,
            stream_id,
            stream_secret,
            approval.invocation_id,
            approval.name,
            approval.input,
            true,
        );
    } else {
        spawn_tool_result_callback(
            runtime,
            fetcher.clone(),
            tx,
            stream_id,
            stream_secret,
            approval.invocation_id,
            approval.name,
            None,
            Some("User rejected the file operation".to_owned()),
            true,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_local_tool(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    guard: WorkspaceGuard,
    tx: mpsc::Sender<ChatEvent>,
    stream_id: String,
    stream_secret: String,
    invocation_id: String,
    name: String,
    input: serde_json::Value,
    render_ui: bool,
) {
    runtime.spawn(async move {
        let result = local_tools::execute(&guard, &name, input);
        let (output, error) = match result {
            Ok(output) => (Some(output), None),
            Err(error) => (None, Some(error)),
        };
        send_tool_result(
            fetcher,
            tx,
            stream_id,
            stream_secret,
            invocation_id,
            name,
            output,
            error,
            render_ui,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
fn spawn_tool_result_callback(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    tx: mpsc::Sender<ChatEvent>,
    stream_id: String,
    stream_secret: String,
    invocation_id: String,
    name: String,
    output: Option<serde_json::Value>,
    error: Option<String>,
    render_ui: bool,
) {
    runtime.spawn(async move {
        send_tool_result(
            fetcher,
            tx,
            stream_id,
            stream_secret,
            invocation_id,
            name,
            output,
            error,
            render_ui,
        )
        .await;
    });
}

#[allow(clippy::too_many_arguments)]
async fn send_tool_result(
    fetcher: Fetcher,
    tx: mpsc::Sender<ChatEvent>,
    stream_id: String,
    stream_secret: String,
    invocation_id: String,
    name: String,
    output: Option<serde_json::Value>,
    error: Option<String>,
    render_ui: bool,
) {
    let ui_output = output
        .as_ref()
        .map(|value| agentic_tools::sanitized_output(&name, value));
    let ui_error = error.clone();
    let params = ToolResultParams {
        stream_id,
        stream_secret,
        invocation_id: invocation_id.clone(),
        output,
        error,
    };
    let callback_error = fetcher
        .tools_result(params)
        .await
        .err()
        .map(|error| format!("Tool result callback failed: {error}"));
    if render_ui || callback_error.is_some() {
        let _ = tx.send(ChatEvent::LocalToolFinished {
            invocation_id,
            name,
            output: if callback_error.is_none() {
                ui_output
            } else {
                None
            },
            error: callback_error.or(ui_error),
        });
    }
}

fn stream_auth(app: &App) -> Option<(String, String)> {
    Some((app.stream_id.clone()?, app.stream_secret.clone()?))
}

fn init_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    // Mouse capture is intentionally OFF so users can select text natively
    // (drag-to-highlight + Cmd/Ctrl+C). Up/Down keys still scroll the chat.
    execute!(stdout, EnterAlternateScreen)?;
    let _ = stdout.write_all(CURSOR_COLOR_SET.as_bytes());
    let _ = stdout.flush();
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn restore_terminal() -> Result<()> {
    let mut stdout = io::stdout();
    let _ = stdout.write_all(CURSOR_COLOR_RESET.as_bytes());
    let _ = stdout.flush();
    disable_raw_mode()?;
    execute!(stdout, LeaveAlternateScreen)?;
    Ok(())
}

pub fn install_panic_hook() {
    let original_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let _ = restore_terminal();
        original_hook(panic_info);
    }));
}
