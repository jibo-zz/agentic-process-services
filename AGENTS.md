# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` owns members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the hosted Axum service, `apps/cli` is the local Ratatui/Crossterm TUI.
- Shared crates live in `crates/`: `agentic-protocol` for API/SSE/RPC DTOs, `agentic-db` for SeaORM persistence, `agentic-tools` for tool contracts/local executors/guardrails.
- Do not couple `apps/server` and `apps/cli` directly; move shared logic into `crates/`.

## Boundaries
- Keep entrypoints thin; `main.rs` installs runtime/error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state/input, `tui.rs` owns the event loop and async task spawning, `ui/<screen>.rs` owns rendering.
- `apps/cli/src/client.rs` exposes typed wrappers (`check_health`, `chat_stream`, `sessions_list`, `sessions_get`, `tools_list`, `tools_result`); raw JSON-RPC stays private behind `rpc_call<T>()`.
- The CLI must not depend on Rig, Open-Meteo, or provider-specific tool types. It executes generic local tool requests and renders generic protocol events.

## Chat Streaming
- Chat uses `POST /chat/stream` SSE, not JSON-RPC. JSON-RPC remains for non-streaming calls on `POST /rpc`.
- The server loads prior turns from Postgres using `ChatRequest { session_id, message }`; clients do not send chat history.
- The TUI loop uses `event::poll(50ms)` + `std::sync::mpsc`; spawn tokio work via `runtime.spawn()` and drain with `try_recv()` before drawing.
- Tool UI parts are upserted by tool-call id. Auto-approved local tools should rely on Rig `ToolCall` / `ToolResult` events for visible blocks; render bridge-only blocks for approval-required operations.
- Persist assistant turns as a side-channel from sanitized stream events. Never block the SSE stream on DB writes.

## Hybrid Tool Runtime
- Rig runs on the server; local filesystem tools execute in the CLI. Never attach real filesystem tools directly to the hosted server.
- `agentic-tools` owns names, schemas, `ToolDescriptor`s, execution kind, risk, output policy, approval requirement, `WorkspaceGuard`, and local executor routing.
- Execution kinds are explicit: `ServerNative` runs in `apps/server`; `LocalProxy` is advertised by the server and executed by the CLI through `ToolBridge`.
- The bridge uses `StreamReady { stream_id, stream_secret }`, `LocalToolRequest` over SSE, and `tools.result` JSON-RPC callbacks. Do not persist real stream secrets.
- Full local tool outputs may be returned to Rig in memory, but UI/database persistence must store summaries only. Do not persist full source files or absolute local paths on the hosted server.
- Rig tool loops need an explicit multi-turn budget (`AGENT_MAX_TOOL_TURNS`) on both the agent and streaming prompt request.

## Local Tool Safety
- The CLI workspace root is `std::env::current_dir().canonicalize()` at startup.
- All local path tools must go through `WorkspaceGuard`; block absolute paths, `..`, symlink escapes, `.git`, `target`, `.env*`, `.pem`, and `.key`.
- Read-only tools run automatically. Mutating tools require inline CLI `Y/N` approval.
- `delete_file` deletes files only. `delete_directory` deletes empty directories only, refuses files, refuses the workspace root, and is not recursive.
- Do not add `run_command` or recursive deletion without a separate design pass for process control, timeouts, and stronger approval.

## Tools Page And Creation
- `/tools` is the tool control center. It merges server `tools.list` with the local `agentic-tools` registry and shows `Active`, `MissingLocally`, or `MissingRemotely` status.
- Keep `/tools` rendering in `apps/cli/src/ui/tools.rs`; keep route/input/state handling in `app.rs`.
- New compiled Rust tools require code changes, rebuild, and restart. Runtime plugins are deferred until there is a sandboxing/signing design.
- Agent-created tools require two approvals before edits: conceptual proposal, then implementation review listing exact files to modify/create.
- Explicit user placement overrides heuristics unless unsafe. Default local filesystem/workspace tools to `LocalProxy`; public API or hosted-secret tools to `ServerNative`.

## Persistence
- Sessions and messages live in Postgres. `messages.parts` is `JSONB` storing `Vec<UiPart>`.
- Schema sync runs on server boot in `agentic_db::connect()` using SeaORM `create_table_from_entity(...).if_not_exists()`. No migration crate yet.
- `sessions::load_history()` returns text-only model history; `sessions::load_ui_messages()` returns rich UI messages. Reasoning/tool/UI errors persist but are not sent to the model.
- SeaORM is pinned to `=2.0.0-rc.38` until stable 2.0 lands.

## UX Notes
- Routes: `/home`, `/sessions`, `/tools`; unknown routes render Not Found.
- `Enter` submits; `Ctrl+Enter` / `Alt+Enter` insert newlines.
- Esc on Sessions and Tools returns Home; elsewhere it quits unless an inline input/approval consumes it.
- Mouse capture is intentionally off so terminal-native selection and copy work. Use arrow keys for chat scrolling.

## Environment And Commands
- Server loads `apps/server/.env`; `DEEPSEEK_API_KEY` and `DATABASE_URL` are required for normal chat operation.
- CLI uses `AGENTS_SERVER_URL` for remote/staging, defaulting to `http://127.0.0.1:3000`.
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server`
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period — e.g. `Build Hybrid Coding Agent Tool Runtime`.
