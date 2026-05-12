# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` is the source of truth for members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the Axum HTTP service, `apps/cli` is the Ratatui/Crossterm TUI.
- Shared libraries live in `crates/`; do not couple `apps/server` and `apps/cli` directly.
- `crates/core` holds shared domain constants and future domain logic used across runnable apps.
- `crates/config` holds shared configuration loading; server address configuration currently comes from `APS_SERVER_ADDR` and defaults to `127.0.0.1:3000`.
- `crates/protocol` holds shared API/client DTOs used by the server and future CLI clients, such as the `/health` response shape.

## Code Organization
- Use Rust module filenames in `snake_case`; keep module names focused on responsibility, not implementation detail.
- Keep app entrypoints thin: `main.rs` should install error/panic handling and delegate to product modules.
- In `apps/cli`, prefer the current separation: `app` for state/input handling, `tui` for terminal setup/event loop, and `ui` modules for rendering/widgets.
- Put screen-specific Ratatui widgets and layout helpers under `ui/<screen>.rs`; only expose the small render surface needed by callers.
- For CLI navigation, prefer typed slash commands like `/home`, `/about`, and `/settings` over shortcut-only page switching; unknown slash routes should render a Not Found page for that route.
- For textarea-like input, keep keybindings explicit in app state; current convention is `Enter` submits and `Ctrl+Enter`/`Alt+Enter` insert newlines.
- Avoid letting product apps share code with each other directly; move reusable config, protocol, or domain code into `crates/`.

## Commands
- Check everything: `cargo check --workspace`.
- Run tests: `cargo test --workspace`; focused tests use `cargo test -p server <test_name>` or `cargo test -p cli <test_name>`.
- Formatting check: `cargo fmt --all -- --check`; apply formatting with `cargo fmt --all`.
- Lint all targets: `cargo clippy --workspace --all-targets`.
- Run the server: `cargo run -p server`; it binds `127.0.0.1:3000` and exposes `GET /health`.
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
