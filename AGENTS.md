# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` is the source of truth for members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the Axum HTTP service, `apps/cli` is the Ratatui/Crossterm TUI.
- Shared libraries live in `crates/`; do not couple `apps/server` and `apps/cli` directly.
- `crates/core` — domain constants. `crates/config` — env/config loading (`APS_SERVER_ADDR`, default `127.0.0.1:3000`). `crates/protocol` — shared DTOs and RPC method names. `crates/db` — SeaORM entities + repos.

## Code Organization
- Keep app entrypoints thin; `main.rs` installs error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state and input logic, `tui.rs` owns the terminal event loop, `ui/<screen>.rs` owns rendering. Don't blur these boundaries.
- CLI navigation uses slash commands (`/home`, `/sessions`); unknown routes render a Not Found page. Active routes: `Home`, `Chat`, `Sessions`, `Missing`.
- `Enter` submits the textarea; `Ctrl+Enter` / `Alt+Enter` insert newlines. Keybindings are declared explicitly in `app.rs`.
- Move any code shared between `apps/server` and `apps/cli` into a `crates/` library.
- `apps/cli/src/client.rs` exposes typed wrapper methods (`check_health`, `chat_stream`, `sessions_list`, `sessions_get`). Raw RPC construction stays private to the client layer via `rpc_call<T>()`. RPC constants and DTOs belong in `agentic-protocol`.
- Server tool implementations stay server-only (`apps/server/src/tools.rs`); CLI renders generic `ChatStreamEvent` / `UiPart` data and must not depend on Rig, Open-Meteo, or provider-specific tool types.

## Streaming
- JSON-RPC can't stream (one request, one response) — use a dedicated SSE endpoint instead. Path constant and event DTO live in `agentic-protocol`; SSE wire-format parsing stays inside the client wrapper.
- `POST /chat/stream` takes `ChatRequest { session_id, message }` — the server loads prior turns from Postgres, not from the client. Wire stays small and history can't be tampered with.
- `/chat/stream` emits `ChatStreamEvent` SSE payloads (not raw text chunks) so reasoning, text, tool updates, and non-terminal errors all render in-place in the TUI.
- The TUI event loop uses `event::poll(50ms)` + `std::sync::mpsc`; spawn tokio tasks via `runtime.spawn()`, drain with `try_recv()` before `terminal.draw()`.
- Guard task spawn with `app.chat_stream().is_pending()` to prevent double-spawning. Set `chat_rx = None` when navigating away — the spawned task exits naturally when its sender errors.
- Tool updates are upserted by tool-call id while text/reasoning deltas append to the last matching part; this avoids tool-block flicker and preserves scroll stability.
- Server-side persistence is a side-channel on the SSE stream: `inspect()` clones each `ChatStreamEvent` into a `tokio::sync::mpsc::unbounded_channel`, a spawned task accumulates via `UiMessage::apply_stream_event()`, and commits the assistant turn on `MessageDone`. Don't block the SSE stream on DB writes.

## Session Persistence (Postgres / SeaORM)
- Sessions and messages live in Postgres. `agentic-db` owns entities and repo functions; `messages.parts` is `JSONB` storing `Vec<UiPart>`.
- Schema sync runs on server boot inside `agentic_db::connect()` via `Schema::create_table_from_entity(...).if_not_exists()`. No migration crate. Adding a column to an entity makes it appear on next startup.
- `sessions::upsert` uses `ON CONFLICT … DO UPDATE SET updated_at = …`. Don't use `do_nothing()`: SeaORM 2.0-rc raises `DbErr::RecordNotInserted` when no rows are affected. A `match` on that variant in the repo function covers any remaining edge cases.
- `sessions::load_history()` returns text-only `Vec<ChatMessage>` for the LLM; `sessions::load_ui_messages()` returns the rich `Vec<UiMessage>` for the Sessions screen. Reasoning, tool state, and UI errors are persisted but not sent to the model.
- `DatabaseConnection` is re-exported from `agentic-db` so `apps/server` doesn't need a direct `sea-orm` dependency.
- SeaORM is pinned to `=2.0.0-rc.38` — no stable 2.0 yet. Bump when 2.0 stable lands.
- Session IDs are still CLI-generated zero-padded 16-digit hex Unix milliseconds (`apps/cli/src/sessions.rs::Session::new`). The CLI sends the id; the server upserts it on first message.
- Esc on the Sessions screen navigates to `Route::Home`, not quit.
- Sessions list and session-open load asynchronously via the same mpsc+drain pattern as chat streaming (see `apps/cli/src/tui.rs`); the loading state is rendered as "Loading sessions…".

## CLI Mouse / Selection
- Mouse capture is intentionally OFF in `apps/cli/src/tui.rs::init_terminal` so drag-select + `Cmd+C` (or `Ctrl+Shift+C`) work via the terminal natively. Cost: mouse wheel doesn't scroll chat. Up/Down arrows scroll instead; footer reads `↑↓ scroll`.
- Crossterm's `EnableMouseCapture` is all-or-nothing — there's no scroll-only mode. Don't try to re-enable it without an explicit toggle UI, because it breaks drag-select.

## Tools
- `get_current_weather` is a single Rig tool that internally geocodes and fetches weather via Open-Meteo to keep the agent loop short; Celsius is the default unless the user explicitly asks for Fahrenheit.
- Rig 0.36 agent streams expose completed tool calls/results and reasoning/text events; tool-call delta UI is still covered by `debug:tool-stream` because the public agent stream path may not yield deltas without hooks.
- `debug:tool-stream` is an exact prompt trigger for testing reasoning, tool states, non-terminal errors, and final text over the real SSE/client path without spending model credits. It also bypasses the DB persistence side-channel.

## Environment
- The server loads `apps/server/.env` via `dotenvy::from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join(".env"))` — works regardless of the working directory `cargo run` is invoked from.
- `DEEPSEEK_API_KEY` must be set for the LLM streaming endpoint.
- `DATABASE_URL` must be a Postgres connection string (Neon works); tables are auto-created on first server boot via schema sync.

## Commands
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server` → binds `127.0.0.1:3000`, exposes `GET /health`, `POST /chat/stream`, `POST /rpc` (methods: `health.check`, `sessions.list`, `sessions.get`).
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period — e.g. `Build agent-style CLI home screen`.

## Runtime Notes
- Server logging: `tracing_subscriber` with `RUST_LOG`; default filter `server=info`.
- Client errors surface response body via `FetchError::Decode("HTTP <status>: <body>")` — don't use `error_for_status()` in the client, it discards the body and hides server-side error context.
- No README, CI, or task runner yet; prefer Cargo commands over project scripts.
- Release profile: `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
