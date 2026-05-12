# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` is the source of truth for members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the Axum HTTP service, `apps/cli` is the Ratatui/Crossterm TUI.
- Shared libraries live in `crates/`; do not couple `apps/server` and `apps/cli` directly.
- `crates/core` holds shared domain constants and future domain logic used across runnable apps.
- `crates/config` holds shared configuration loading; server address configuration comes from `APS_SERVER_ADDR` and defaults to `127.0.0.1:3000`.
- `crates/protocol` holds shared API/client DTOs and JSON-RPC method names used by the server and CLI, such as health and LLM response shapes.

## Code Organization
- Use Rust module filenames in `snake_case`; keep module names focused on responsibility, not implementation detail.
- Keep app entrypoints thin: `main.rs` should install error/panic handling and delegate to product modules.
- In `apps/cli`, prefer the current separation: `app` for state/input handling, `tui` for terminal setup/event loop, and `ui` modules for rendering/widgets.
- Put screen-specific Ratatui widgets and layout helpers under `ui/<screen>.rs`; only expose the small render surface needed by callers.
- For CLI navigation, prefer typed slash commands like `/home` and `/sessions` over shortcut-only page switching; unknown slash routes should render a Not Found page for that route. Active routes are: `Home`, `Chat`, `Sessions`, `Missing`.
- For textarea-like input, keep keybindings explicit in app state; current convention is `Enter` submits and `Ctrl+Enter`/`Alt+Enter` insert newlines.
- Avoid letting product apps share code with each other directly; move reusable config, protocol, or domain code into `crates/`.
- When `apps/cli` needs server data, use typed client-wrapper methods such as `check_health()` or `chat_stream()` from `apps/cli/src/client.rs`. Those wrappers should call the existing JSON-RPC endpoint whenever practical, while keeping raw `RpcRequest`, method IDs/names, and `/rpc` request construction private to the client layer. Keep RPC method constants, request/response DTOs, and shared error shapes in `agentic-protocol`.
- For streaming responses JSON-RPC is not practical (one request = one response); use a dedicated SSE endpoint instead. The path constant and chunk DTO belong in `agentic-protocol`; the HTTP/SSE parsing stays private to the client wrapper method.
- SSE streaming uses a POST body (not GET query params) when the request carries conversation history. The endpoint is `POST /chat/stream` with a `ChatRequest { messages: Vec<ChatMessage> }` JSON body; the last message must have `role: "user"`. This is declared in `agentic-protocol` as `CHAT_STREAM_PATH`.
- SSE wire-format parsing in `client.rs` uses `stream::unfold` over the raw bytes stream, accumulating a string buffer and splitting on `"\n\n"` event boundaries, then stripping the `"data: "` prefix and deserializing each payload as `LlmChunk`. All parsing stays inside the `chat_stream()` method; callers receive `impl Stream<Item = Result<LlmChunk, FetchError>>`.
- In `apps/cli`, the TUI event loop uses `event::poll(Duration::from_millis(50))` rather than blocking `event::read()` so background streaming tasks can drain their `std::sync::mpsc` channel and re-render each tick. Spawn tokio tasks via `runtime.spawn()` from the synchronous event loop; send results through `mpsc::channel`; drain with `try_recv()` before `terminal.draw()`.
- The streaming task is guarded by `app.chat_stream().is_pending()` — a single `Pending(String)` state transition into `Streaming { user_msg, response }` prevents double-spawning. Drop `chat_rx` (set to `None`) when the user navigates away; the spawned task's `tx.send()` will fail and exit naturally without explicit cancellation.
- For widgets that need to normalize or write back state during render (e.g. clamping a scroll offset), pass `&mut App` through `ui::render` and the widget struct. The widget computes the clamped value and writes it back via a setter so keyboard and mouse scroll work correctly from the actual displayed position.
- Mouse capture must be enabled/disabled in `init_terminal`/`restore_terminal` via `event::EnableMouseCapture` / `event::DisableMouseCapture`; handle `Event::Mouse(MouseEventKind::ScrollUp/ScrollDown)` in the same event `match` as key events.

## Session Persistence
- `apps/cli/src/sessions.rs` owns the `Session` and `Turn` types plus `save()` / `load_all()` helpers.
- Sessions are stored as JSON files in `~/.faaido/sessions/<id>.json`. The `id` is a zero-padded 16-digit hex representation of the Unix timestamp in milliseconds, which also gives natural chronological sort order.
- `Session::to_chat_messages(new_user_msg: Option<&str>)` builds the flat `Vec<ChatMessage>` for the API by interleaving all completed `Turn` pairs (user then assistant) and appending the optional new user message. This is the canonical way to produce the history slice for `POST /chat/stream`.
- `load_all()` reads every `.json` file in the sessions directory and returns them sorted by `created_at` descending (newest first). Corrupted or unreadable files are silently skipped.
- The `/sessions` slash command in `App::handle_route_command` calls `sessions::load_all()` and sets `Route::Sessions`. Esc on the Sessions screen navigates back to `Route::Home` rather than quitting.

## Environment and Configuration
- The server loads its `.env` file at `apps/server/.env` using `dotenvy::from_path(manifest_dir.join(".env"))` where `manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))`. This resolves relative to the server crate at compile time and works regardless of the working directory from which `cargo run` is invoked.
- `DEEPSEEK_API_KEY` must be set (via `.env` or environment) for the LLM streaming endpoint to succeed.
- `.gitignore` covers `**/.env`, `**/.env.local`, `**/.env.*.local`, `.DS_Store`, `.idea/`, `.vscode/`.

## Commands
- Check everything: `cargo check --workspace`.
- Run tests: `cargo test --workspace`; focused tests use `cargo test -p server <test_name>` or `cargo test -p cli <test_name>`.
- Formatting check: `cargo fmt --all -- --check`; apply formatting with `cargo fmt --all`.
- Lint all targets: `cargo clippy --workspace --all-targets`.
- Run the server: `cargo run -p server`; it binds `127.0.0.1:3000` and exposes `GET /health`, `POST /chat/stream`, and `POST /rpc`.
- Run the server on another address: `APS_SERVER_ADDR=127.0.0.1:4000 cargo run -p server`.
- Run the TUI: `cargo run -p cli`; it takes over the terminal alternate screen and exits on `Esc` or `Ctrl+C`.

## Git
- Match the latest commit subject format for new commits: a short, imperative, title-cased one-line summary with no trailing period, like `Build agent-style CLI home screen`.

## Runtime Notes
- Server logging uses `tracing_subscriber` with `RUST_LOG` support and defaults to `server=info`.
- The server returns shared protocol DTOs from `agentic-protocol`; keep API response structs there if the CLI will consume them later.
- Keep config parsing in `agentic-config` when both apps may need the same environment variables or defaults.
- Keep product/domain names and cross-app constants in `agentic-core` to avoid duplicated strings.
- There is no repo-local README, CI, task runner, formatter config, or pre-commit config yet; prefer Cargo commands over inventing project scripts.
- Release profile is optimized for installable binaries (`lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true`).
