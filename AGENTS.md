# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` is the source of truth for members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the Axum HTTP service, `apps/cli` is the Ratatui/Crossterm TUI.
- Shared libraries live in `crates/`; do not couple `apps/server` and `apps/cli` directly.
- `crates/core` — shared domain constants. `crates/config` — shared env/config loading (`APS_SERVER_ADDR`, default `127.0.0.1:3000`). `crates/protocol` — shared API DTOs and RPC method names.

## Code Organization
- Keep app entrypoints thin; `main.rs` installs error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state and input logic, `tui.rs` owns the terminal event loop, `ui/<screen>.rs` owns rendering. Don't blur these boundaries.
- CLI navigation uses slash commands (`/home`, `/sessions`); unknown routes render a Not Found page. Active routes: `Home`, `Chat`, `Sessions`, `Missing`.
- `Enter` submits the textarea; `Ctrl+Enter` / `Alt+Enter` insert newlines. Keybindings are declared explicitly in `app.rs`.
- Move any code shared between `apps/server` and `apps/cli` into a `crates/` library.
- `apps/cli/src/client.rs` exposes typed wrapper methods (`check_health()`, `chat_stream()`). Raw RPC construction stays private to the client layer. RPC constants and DTOs belong in `agentic-protocol`.

## Streaming
- JSON-RPC can't stream (one request, one response) — use a dedicated SSE endpoint instead. Path constant and chunk DTO live in `agentic-protocol`; SSE wire-format parsing stays inside the client wrapper.
- The streaming endpoint is `POST /chat/stream` with a `ChatRequest { messages }` JSON body (not GET + query param) because conversation history must travel in the request body.
- The TUI event loop uses `event::poll(50ms)` + `std::sync::mpsc`; spawn tokio tasks via `runtime.spawn()`, drain with `try_recv()` before `terminal.draw()`.
- Guard task spawn with `app.chat_stream().is_pending()` to prevent double-spawning. Set `chat_rx = None` when navigating away — the spawned task exits naturally when its sender errors.

## Session Persistence
- Sessions are stored as JSON in `~/.faaido/sessions/<id>.json`. The `id` is a zero-padded 16-digit hex Unix timestamp in milliseconds.
- `Session::to_chat_messages(new_user_msg)` is the only correct way to build the history slice for `POST /chat/stream`; it interleaves completed turns then appends the optional new user message.
- Esc on the Sessions screen navigates to `Route::Home`, not quit.

## Environment
- The server loads `apps/server/.env` via `dotenvy::from_path(Path::new(env!("CARGO_MANIFEST_DIR")).join(".env"))` — works regardless of the working directory `cargo run` is invoked from.
- `DEEPSEEK_API_KEY` must be set for the LLM streaming endpoint.

## Commands
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server` → binds `127.0.0.1:3000`, exposes `GET /health`, `POST /chat/stream`, `POST /rpc`
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period — e.g. `Build agent-style CLI home screen`.

## Runtime Notes
- Server logging: `tracing_subscriber` with `RUST_LOG`; default filter `server=info`.
- No README, CI, or task runner yet; prefer Cargo commands over project scripts.
- Release profile: `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`.
