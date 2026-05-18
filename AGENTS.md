# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` owns members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the hosted Axum service, `apps/cli` is the local Ratatui/Crossterm TUI.
- Shared crates live in `crates/`: `agentic-protocol` for API/SSE/RPC DTOs, `agentic-db` for SeaORM persistence, `agentic-tools` for tool contracts, the subprocess runner, and guardrails.
- Do not couple `apps/server` and `apps/cli` directly; move shared logic into `crates/`.

## Boundaries
- Keep entrypoints thin; `main.rs` installs runtime/error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state/input, `tui.rs` owns the event loop and async task spawning, `ui/<screen>.rs` owns rendering.
- `apps/cli/src/client.rs` exposes typed wrappers (`check_health`, `chat_stream`, `sessions_list`, `sessions_get`, `tools_list`, `tools_result`, `tools_management`, `tools_save_draft`, `tools_register`, `tools_delete_version`); raw JSON-RPC stays private behind `rpc_call<T>()`.
- The CLI must not depend on Rig, Open-Meteo, or provider-specific tool types. It executes generic local tool requests and renders generic protocol events.

## Chat Streaming
- Chat uses `POST /chat/stream` SSE, not JSON-RPC. JSON-RPC remains for non-streaming calls on `POST /rpc`.
- The server loads prior turns from Postgres using `ChatRequest { session_id, message }`; clients do not send chat history.
- The TUI loop uses `event::poll(50ms)` + `std::sync::mpsc`; spawn tokio work via `runtime.spawn()` and drain with `try_recv()` before drawing.
- Tool UI parts are upserted by tool-call id. Auto-approved local tools rely on Rig `ToolCall` / `ToolResult` events for visible blocks; render bridge-only blocks for approval-required operations.
- Persist assistant turns as a side-channel from sanitized stream events. Never block the SSE stream on DB writes.

## Two-Tier Tool Runtime
- **Tier-1** — compiled Rust tools in `agentic_tools::registry()`. Stable, deeply-typed, performance-critical. Adding or editing a Tier-1 tool requires code changes, rebuild, and restart.
- **Tier-2** — Python or shell scripts stored as versioned rows in Postgres (`tools` / `tool_versions`). Added, edited, tested, and activated at runtime with no rebuild and no server restart. Executed in the CLI via subprocess.
- The server merges both tiers in `tools.list`; the agent sees one flat list, tier is an implementation detail. Tier-2 tools are advertised as `LocalProxy` regardless of language.
- Tier-2 script bodies travel inline in `LocalToolRequest.script`; the server stays free of script execution. Never persist Tier-2 script bodies in UI history — `sanitize_event_for_persistence` strips the `script` field. Full local tool outputs may be returned to Rig in memory, but UI/database persistence must store summaries only. Do not persist full source files or absolute local paths on the hosted server.
- Dynamic Rig integration for Tier-2 uses `DynamicProxyTool` with a placeholder `NAME` constant and an overridden `name()` — Rig keys its `ToolSet` by `name()`, not `NAME`. Registered per-stream via `agent.tools(Vec<Box<dyn ToolDyn>>)`.
- Rig tool loops need an explicit multi-turn budget (`AGENT_MAX_TOOL_TURNS`) on both the agent and streaming prompt request.

## Tier-2 Script Runner
- `agentic_tools::runner::run` spawns `python3 -` or `sh -s`, pipes the script to stdin, sets `ARGS_JSON` env to the JSON input, and waits with `tokio::time::timeout`. `kill_on_drop(true)` plus explicit `start_kill()` on overrun guarantee the child dies on timeout.
- PyO3 is rejected on purpose: an embedded CPython would forfeit timeout-kill (GIL/signal limits), let C-extension segfaults take down the host, and require a build-time Python dependency. Subprocess gives a real process boundary and one executor for both Python and shell.
- Rust will never be a Tier-2 source language. Source-Rust tools live in Tier-1; hot-loadable Rust comes via a future `wasm-component` artifact variant in `tool_versions.language`, not by accepting Rust source.
- Read stdout/stderr concurrently with stdin writes so a chatty script does not deadlock the pipe. Caps: 1 MiB stdout / 256 KiB stderr; truncate beyond and flag.

## Local Tool Safety
- The CLI workspace root is `std::env::current_dir().canonicalize()` at startup.
- All local path tools must go through `WorkspaceGuard`; block absolute paths, `..`, symlink escapes, `.git`, `target`, `.env*`, `.pem`, `.key`, and `.agent-tools/`.
- Read-only tools run automatically. Mutating tools and any non-`ReadOnly` Tier-2 tool require inline CLI `Y/N` approval; `PendingToolApproval` carries the optional script through the approval gate so the same code path handles Tier-1 and Tier-2 confirmations.
- Tier-2 scratch directory is `<workspace>/.agent-tools/scratch/<uuid>/`: created on demand, set as cwd and `TMPDIR`, removed after the run. Env is scrubbed to `PATH`, `LANG`, `TMPDIR`, `ARGS_JSON`. v1 isolation is trust-the-user (workspace + wall-clock timeout); OS-level sandboxing (Landlock / sandbox-exec) is a phase-2 add.
- `delete_file` deletes files only. `delete_directory` deletes empty directories only, refuses files, refuses the workspace root, and is not recursive.
- Do not add `run_command` or recursive deletion without a separate design pass for process control, timeouts, and stronger approval.

## Tools Page And Editor
- `/tools` is the tool control center. It merges server `tools.list` (static + active DB tools) with the local `agentic-tools` registry and shows `Active`, `MissingLocally`, or `MissingRemotely` status.
- Pressing `N` opens the Tool Editor sub-mode. Fields: Name, Script (multi-line), ARGS_JSON. `F2` toggles Python/Shell. `Tab`/`Shift+Tab` cycle focus. `Ctrl+R` runs the script via the subprocess runner. `Ctrl+S` saves a draft version. `Ctrl+P` publishes the last saved draft (server-side `register_active` demotes the prior active in one transaction). After publish, `tools.list` is auto-refreshed so the new tool surfaces to the next chat turn without a server restart.
- Keep `/tools` rendering in `apps/cli/src/ui/tools.rs`; keep route/input/state handling in `app.rs`; keep editor action dispatch in `tui.rs`.
- v1 risk default for editor-authored tools is `ReadOnly` (no approval). Author riskier tools by direct DB writes or by extending the editor UI.
- Agent-created tools require two approvals before edits: conceptual proposal, then implementation review listing exact files to modify/create.
- Explicit user placement overrides heuristics unless unsafe. Default local filesystem/workspace tools to Tier-1 `LocalProxy`; public API or hosted-secret tools to `ServerNative`. Script-authored Python/shell tools go to Tier-2.

## Persistence
- Sessions and messages live in Postgres. `messages.parts` is `JSONB` storing `Vec<UiPart>`.
- Tier-2 tools live in `tools` (one row per logical name, points at `current_version_id`) and `tool_versions` (immutable per-version rows with `language`, `script`, `args_schema`, `output_schema`, `tests`, `status: draft|active|deprecated`, `risk`, `timeout_ms`). Version numbers are monotonic per tool.
- Schema sync runs on server boot in `agentic_db::connect()` using SeaORM `create_table_from_entity(...).if_not_exists()`. No migration crate yet.
- `sessions::load_history()` returns text-only model history; `sessions::load_ui_messages()` returns rich UI messages. Reasoning/tool/UI errors persist but are not sent to the model.
- SeaORM is pinned to `=2.0.0-rc.38` until stable 2.0 lands.

## UX Notes
- Routes: `/home`, `/sessions`, `/tools`; unknown routes render Not Found.
- `Enter` submits; `Ctrl+Enter` / `Alt+Enter` insert newlines. In the Tool Editor's Script field, `Enter` always inserts a newline.
- Esc on Sessions and Tools returns Home; in the Tool Editor, Esc returns to the tools list; elsewhere Esc quits unless an inline input/approval consumes it.
- Mouse capture is intentionally off so terminal-native selection and copy work. Up/Down scroll the chat; Left/Right/Home/End/Delete edit the text-input caret.

## Environment And Commands
- Server loads `apps/server/.env`; `DEEPSEEK_API_KEY` and `DATABASE_URL` are required for normal chat operation.
- CLI uses `AGENTS_SERVER_URL` for remote/staging, defaulting to `http://127.0.0.1:3000`.
- Tier-2 Python tools require `python3` on the CLI host's PATH at runtime; no build-time Python dependency.
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server`
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period — e.g. `Build Hybrid Coding Agent Tool Runtime`.
