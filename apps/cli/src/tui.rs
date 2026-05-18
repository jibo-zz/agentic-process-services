use crate::{
    app::{
        App, GenerationState, PendingToolApproval, ToolEditorAction, ToolEditorResult,
        ToolEditorResultKind, ToolEditorSnapshot,
    },
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
    AuthorRequest, ChatStreamEvent, LocalToolScript, SaveDraftParams, SessionSummary,
    ToolResultParams, ToolRisk, ToolVersionRow, ToolsListResponse, UiMessage,
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

enum EditorEvent {
    RunResult(ToolEditorResult),
    DraftSaved {
        row: ToolVersionRow,
    },
    Registered {
        row: ToolVersionRow,
    },
    Error(String),
    ToolDeleted {
        name: String,
    },
    ToolDeleteFailed {
        name: String,
        error: String,
    },
    GenerationStarted,
    GenerationProgress {
        line: String,
    },
    GenerationDone {
        version_id: String,
        name: String,
        language: agentic_protocol::ToolScriptLanguage,
        script: String,
        args_schema: serde_json::Value,
    },
    GenerationFailed(String),
}

type SessionsOpenResult = Result<(String, Vec<UiMessage>), FetchError>;
type ToolsListResult =
    Result<(ToolsListResponse, agentic_protocol::ToolsManagementResponse), FetchError>;

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
    let mut editor_rx: Option<mpsc::Receiver<EditorEvent>> = None;

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
                let _ = tx.send(load_tools(&f).await);
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
                Ok((list, mgmt)) => app.set_tools_from_server(list.tools, mgmt.tools),
                Err(error) => {
                    app.set_tools_error(format!("Server tool registry unavailable: {error}"))
                }
            }
            tools_rx = None;
        }

        // Drain editor task results. When the sender drops (one-shot task done) clear
        // `editor_rx` so the next editor action can take the slot — otherwise subsequent
        // dispatches are silently dropped.
        let mut editor_rx_done = false;
        if let Some(ref rx) = editor_rx {
            loop {
                match rx.try_recv() {
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        editor_rx_done = true;
                        break;
                    }
                    Ok(event) => match event {
                        EditorEvent::RunResult(result) => app.set_editor_result(result),
                        EditorEvent::DraftSaved { row } => {
                            app.set_last_draft_version_id(row.id.clone());
                            app.set_editor_result(ToolEditorResult {
                                kind: ToolEditorResultKind::Success,
                                message: format!("Saved draft v{} ({})", row.version, row.id),
                                stdout: String::new(),
                                stderr: String::new(),
                                exit_code: None,
                                duration_ms: 0,
                            });
                        }
                        EditorEvent::Registered { row } => {
                            app.set_editor_result(ToolEditorResult {
                                kind: ToolEditorResultKind::Success,
                                message: format!(
                                    "Registered '{}' v{} as active",
                                    short_id(&row.tool_id),
                                    row.version
                                ),
                                stdout: String::new(),
                                stderr: String::new(),
                                exit_code: None,
                                duration_ms: 0,
                            });
                            // refresh the tools list so the new tool surfaces in the table
                            let (tx, rx) = mpsc::channel();
                            tools_rx = Some(rx);
                            let f = fetcher.clone();
                            runtime.spawn(async move {
                                let _ = tx.send(load_tools(&f).await);
                            });
                        }
                        EditorEvent::ToolDeleted { name } => {
                            app.tools_notice = Some(format!("Deleted tool '{name}'."));
                            let (tx, rx) = mpsc::channel();
                            tools_rx = Some(rx);
                            app.tools_loaded = false;
                            let f = fetcher.clone();
                            runtime.spawn(async move {
                                let _ = tx.send(load_tools(&f).await);
                            });
                        }
                        EditorEvent::ToolDeleteFailed { name, error } => {
                            app.tools_notice = Some(format!("Delete '{name}' failed: {error}"));
                        }
                        EditorEvent::Error(message) => {
                            app.set_editor_result(ToolEditorResult {
                                kind: ToolEditorResultKind::Failure,
                                message,
                                stdout: String::new(),
                                stderr: String::new(),
                                exit_code: None,
                                duration_ms: 0,
                            });
                        }
                        EditorEvent::GenerationStarted => {
                            app.clear_generation_log();
                            app.set_generation_state(GenerationState::Generating);
                            app.push_generation_log("Author agent started. Working...");
                        }
                        EditorEvent::GenerationProgress { line } => {
                            app.push_generation_log(line);
                        }
                        EditorEvent::GenerationDone {
                            version_id,
                            name,
                            language,
                            script,
                            args_schema,
                        } => {
                            app.push_generation_log(format!(
                                "Generated draft v_id={} for tool '{}'. Press Ctrl+P to publish.",
                                short_id(&version_id),
                                name
                            ));
                            app.apply_author_done(version_id, name, language, script, args_schema);
                        }
                        EditorEvent::GenerationFailed(reason) => {
                            app.push_generation_log(format!("Generation failed: {reason}"));
                            app.set_generation_state(GenerationState::Failed(reason));
                        }
                    },
                }
            }
        }
        if editor_rx_done {
            editor_rx = None;
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
                    if let Some(action) = app.take_pending_editor_action() {
                        let snapshot = app.editor_snapshot();
                        if editor_rx.is_none() {
                            let (tx, rx) = mpsc::channel();
                            editor_rx = Some(rx);
                            dispatch_editor_action(
                                &runtime,
                                &fetcher,
                                &workspace_guard,
                                tx,
                                action,
                                snapshot,
                            );
                        }
                    }
                    if let Some((target, confirmed)) = app.take_tool_delete_decision()
                        && confirmed
                    {
                        let (tx, rx) = mpsc::channel();
                        editor_rx = Some(rx);
                        let f = fetcher.clone();
                        let tool_id = target.tool_id.clone();
                        let name = target.name.clone();
                        runtime.spawn(async move {
                            match f.tools_delete_tool(tool_id).await {
                                Ok(ack) if ack.deleted => {
                                    let _ = tx.send(EditorEvent::ToolDeleted { name });
                                }
                                Ok(_) => {
                                    let _ = tx.send(EditorEvent::ToolDeleteFailed {
                                        name,
                                        error: "Tool not found on server".to_owned(),
                                    });
                                }
                                Err(error) => {
                                    let _ = tx.send(EditorEvent::ToolDeleteFailed {
                                        name,
                                        error: error.to_string(),
                                    });
                                }
                            }
                        });
                    }
                    if let Some(item) = app.take_pending_reopen_draft() {
                        app.reopen_draft(item);
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
            script,
        } => {
            if approval_required {
                let summary = match &script {
                    Some(_) => format!("Run {name}?"),
                    None => agentic_tools::approval_summary(&name, &input, workspace_guard),
                };
                app.apply_chat_stream_event(ChatStreamEvent::LocalToolRequest {
                    invocation_id: invocation_id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                    approval_required,
                    summary,
                    script: None,
                });
                app.set_pending_tool_approval(PendingToolApproval {
                    invocation_id,
                    name,
                    input,
                    script,
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
                if let Some(script) = script {
                    spawn_tier2_tool(
                        runtime,
                        fetcher.clone(),
                        workspace_guard.clone(),
                        tx,
                        stream_id,
                        stream_secret,
                        invocation_id,
                        name,
                        input,
                        script,
                        false,
                    );
                } else {
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
        if let Some(script) = approval.script {
            spawn_tier2_tool(
                runtime,
                fetcher.clone(),
                workspace_guard.clone(),
                tx,
                stream_id,
                stream_secret,
                approval.invocation_id,
                approval.name,
                approval.input,
                script,
                true,
            );
        } else {
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
        }
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
fn spawn_tier2_tool(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    guard: WorkspaceGuard,
    tx: mpsc::Sender<ChatEvent>,
    stream_id: String,
    stream_secret: String,
    invocation_id: String,
    name: String,
    input: serde_json::Value,
    script: LocalToolScript,
    render_ui: bool,
) {
    runtime.spawn(async move {
        let result = local_tools::execute_tier2(&guard, input, script).await;
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

async fn load_tools(fetcher: &Fetcher) -> ToolsListResult {
    let list = fetcher.tools_list().await?;
    let mgmt = fetcher.tools_management().await?;
    Ok((list, mgmt))
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn dispatch_editor_action(
    runtime: &tokio::runtime::Runtime,
    fetcher: &Fetcher,
    workspace_guard: &WorkspaceGuard,
    tx: mpsc::Sender<EditorEvent>,
    action: ToolEditorAction,
    snapshot: ToolEditorSnapshot,
) {
    match action {
        ToolEditorAction::Generate => spawn_editor_generate(
            runtime,
            fetcher.clone(),
            workspace_guard.clone(),
            tx,
            snapshot,
        ),
        ToolEditorAction::Run => spawn_editor_run(runtime, workspace_guard.clone(), tx, snapshot),
        ToolEditorAction::SaveDraft => {
            spawn_editor_save_draft(runtime, fetcher.clone(), tx, snapshot)
        }
        ToolEditorAction::Register => spawn_editor_register(runtime, fetcher.clone(), tx, snapshot),
    }
}

fn spawn_editor_generate(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    guard: WorkspaceGuard,
    tx: mpsc::Sender<EditorEvent>,
    snapshot: ToolEditorSnapshot,
) {
    runtime.spawn(async move {
        if snapshot.description.is_empty() {
            let _ = tx.send(EditorEvent::GenerationFailed(
                "Describe the tool first.".to_owned(),
            ));
            return;
        }
        let req = AuthorRequest {
            description: snapshot.description,
            input_hint: Some(snapshot.input_hint).filter(|s| !s.is_empty()),
            output_hint: Some(snapshot.output_hint).filter(|s| !s.is_empty()),
        };
        let stream = match fetcher.author_stream(req).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = tx.send(EditorEvent::GenerationFailed(error.to_string()));
                return;
            }
        };
        let _ = tx.send(EditorEvent::GenerationStarted);
        tokio::pin!(stream);

        let mut stream_id = String::new();
        let mut stream_secret = String::new();
        let mut finished = false;

        while let Some(item) = stream.next().await {
            let event = match item {
                Ok(event) => event,
                Err(error) => {
                    let _ = tx.send(EditorEvent::GenerationFailed(error.to_string()));
                    return;
                }
            };
            match event {
                ChatStreamEvent::StreamReady {
                    stream_id: id,
                    stream_secret: secret,
                } => {
                    stream_id = id;
                    stream_secret = secret;
                }
                ChatStreamEvent::ToolUpdate { name, state, .. } => {
                    let _ = tx.send(EditorEvent::GenerationProgress {
                        line: format!("tool: {name} -> {state:?}"),
                    });
                }
                ChatStreamEvent::ReasoningDelta { .. } => {}
                ChatStreamEvent::TextDelta { text } => {
                    if !text.trim().is_empty() {
                        let _ = tx.send(EditorEvent::GenerationProgress {
                            line: format!("agent: {}", text.trim()),
                        });
                    }
                }
                ChatStreamEvent::LocalToolRequest {
                    invocation_id,
                    name,
                    input,
                    script: Some(script),
                    ..
                } => {
                    let _ = tx.send(EditorEvent::GenerationProgress {
                        line: format!("sandbox_run: {}", short_args(&input)),
                    });
                    let outcome = local_tools::execute_tier2(&guard, input, script).await;
                    let (output, error) = match outcome {
                        Ok(v) => (Some(v), None),
                        Err(e) => (None, Some(e)),
                    };
                    let _ = fetcher
                        .tools_result(ToolResultParams {
                            stream_id: stream_id.clone(),
                            stream_secret: stream_secret.clone(),
                            invocation_id,
                            output,
                            error,
                        })
                        .await;
                    let _ = name;
                }
                ChatStreamEvent::LocalToolRequest {
                    invocation_id,
                    name,
                    ..
                } => {
                    // Tier-1 request would be unexpected here (the author agent has no Tier-1
                    // tools), but ack it so the server's bridge doesn't sit blocked for 600s.
                    let _ = fetcher
                        .tools_result(ToolResultParams {
                            stream_id: stream_id.clone(),
                            stream_secret: stream_secret.clone(),
                            invocation_id,
                            output: None,
                            error: Some(format!(
                                "Tier-1 tool '{name}' is not available during tool authoring"
                            )),
                        })
                        .await;
                }
                ChatStreamEvent::AuthorDone {
                    version_id,
                    name,
                    language,
                    script,
                    args_schema,
                    ..
                } => {
                    let _ = tx.send(EditorEvent::GenerationDone {
                        version_id,
                        name,
                        language,
                        script,
                        args_schema,
                    });
                    finished = true;
                }
                ChatStreamEvent::Error { message } => {
                    let _ = tx.send(EditorEvent::GenerationFailed(message));
                    finished = true;
                }
                ChatStreamEvent::MessageStart { .. } | ChatStreamEvent::MessageDone => {}
            }
        }
        if !finished {
            let _ = tx.send(EditorEvent::GenerationFailed(
                "Generation stream ended without a result.".to_owned(),
            ));
        }
    });
}

fn short_args(value: &serde_json::Value) -> String {
    let s = value.to_string();
    if s.len() > 64 {
        format!("{}…", &s[..63])
    } else {
        s
    }
}

fn spawn_editor_run(
    runtime: &tokio::runtime::Runtime,
    guard: WorkspaceGuard,
    tx: mpsc::Sender<EditorEvent>,
    snapshot: ToolEditorSnapshot,
) {
    runtime.spawn(async move {
        if snapshot.script.trim().is_empty() {
            let _ = tx.send(EditorEvent::Error("Script is empty.".to_owned()));
            return;
        }
        let args: serde_json::Value = serde_json::from_str(&snapshot.args)
            .unwrap_or(serde_json::Value::Object(Default::default()));
        let script = LocalToolScript {
            language: snapshot.language,
            script: snapshot.script,
            timeout_ms: 10_000,
        };
        match local_tools::execute_tier2(&guard, args, script).await {
            Ok(value) => {
                let stdout = value
                    .get("stdout")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let stderr = value
                    .get("stderr")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                let exit_code = value
                    .get("exit_code")
                    .and_then(|v| v.as_i64())
                    .map(|n| n as i32);
                let duration_ms = value
                    .get("duration_ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let message = format!("Ran in {duration_ms}ms");
                let _ = tx.send(EditorEvent::RunResult(ToolEditorResult {
                    kind: ToolEditorResultKind::Success,
                    message,
                    stdout,
                    stderr,
                    exit_code,
                    duration_ms,
                }));
            }
            Err(error) => {
                let _ = tx.send(EditorEvent::RunResult(ToolEditorResult {
                    kind: ToolEditorResultKind::Failure,
                    message: error,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: None,
                    duration_ms: 0,
                }));
            }
        }
    });
}

fn spawn_editor_save_draft(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    tx: mpsc::Sender<EditorEvent>,
    snapshot: ToolEditorSnapshot,
) {
    runtime.spawn(async move {
        if snapshot.name.is_empty() {
            let _ = tx.send(EditorEvent::Error("Tool name is required.".to_owned()));
            return;
        }
        if snapshot.script.trim().is_empty() {
            let _ = tx.send(EditorEvent::Error("Script is empty.".to_owned()));
            return;
        }
        let args_schema: serde_json::Value = serde_json::from_str(&snapshot.args)
            .unwrap_or_else(|_| serde_json::json!({ "type": "object" }));
        let params = SaveDraftParams {
            name: snapshot.name.clone(),
            description: format!("Tier-2 tool '{}'", snapshot.name),
            language: snapshot.language,
            script: snapshot.script,
            args_schema,
            output_schema: None,
            tests: Vec::new(),
            risk: ToolRisk::ReadOnly,
            timeout_ms: 10_000,
            owner: "scratchpad".to_owned(),
        };
        match fetcher.tools_save_draft(params).await {
            Ok(row) => {
                let _ = tx.send(EditorEvent::DraftSaved { row });
            }
            Err(error) => {
                let _ = tx.send(EditorEvent::Error(format!("Save draft failed: {error}")));
            }
        }
    });
}

fn spawn_editor_register(
    runtime: &tokio::runtime::Runtime,
    fetcher: Fetcher,
    tx: mpsc::Sender<EditorEvent>,
    snapshot: ToolEditorSnapshot,
) {
    runtime.spawn(async move {
        let Some(version_id) = snapshot.last_draft_version_id else {
            let _ = tx.send(EditorEvent::Error(
                "Save a draft first (Ctrl+S), then publish.".to_owned(),
            ));
            return;
        };
        match fetcher.tools_register(version_id).await {
            Ok(row) => {
                let _ = tx.send(EditorEvent::Registered { row });
            }
            Err(error) => {
                let _ = tx.send(EditorEvent::Error(format!("Register failed: {error}")));
            }
        }
    });
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
