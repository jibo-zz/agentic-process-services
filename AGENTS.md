# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` owns members and shared dependency versions.
- Runnable products live in `apps/`: `apps/server` is the hosted Axum service, `apps/cli` is the local Ratatui/Crossterm TUI.
- Shared crates live in `crates/`: protocol DTOs, DB persistence, core/config, and `agentic-tools` for tool contracts, instructions, runners, and guardrails.
- Do not couple `apps/server` and `apps/cli` directly; move shared logic into `crates/`.

## Skills
- Use `cargo-rust` for workspace dependency, check, test, format, and lint work.
- Use `ratatui-tui` for CLI layout, input handling, textareas, hotkeys, async event loops, and rendering changes.
- Use `rig` for server agent construction, preambles, tool registration, streaming, and tool-loop behavior.
- Use `axum` for server routing/extractor/middleware changes and `sea-orm` for DB/entity/repo changes.

## Boundaries
- Keep entrypoints thin; `main.rs` installs runtime/error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state/input, `tui.rs` owns the event loop and async task spawning, `ui/<screen>.rs` owns rendering.
- `apps/cli/src/client.rs` exposes typed wrappers; raw JSON-RPC stays private behind `rpc_call<T>()`.
- The CLI must not depend on Rig, Open-Meteo, or provider-specific tool types. It executes generic local tool requests and renders generic protocol events.

## Chat And Modes
- Chat uses `POST /chat/stream` SSE, not JSON-RPC. JSON-RPC remains for non-streaming calls on `POST /rpc`.
- The server loads prior turns from Postgres using `ChatRequest { session_id, message, mode }`; clients do not send chat history.
- `AgentMode` lives in `crates/protocol` and currently supports `Build` and `Plan`. Add future modes by extending `AgentMode::ALL` and policy helpers rather than hardcoding two-mode toggles.
- `BUILD` is the default and allows the full tool set subject to normal approval rules.
- `PLAN` is safe/read-only: only Tier-1 compiled inspection tools are allowed (`list_files`, `read_file`, `search_files`). It blocks mutating tools, `get_current_weather`, all network tools, and all Tier-2 subprocess tools even when they claim `ReadOnly`.
- Mode instructions live in `crates/agent-tools/src/instructions.rs`; server chat assembly must use `coding_agent_preamble(mode)`.
- Server-side defense-in-depth: attach only mode-allowed Rig tools and reject disallowed calls in `LocalToolContext` before bridge dispatch.
- CLI-side defense-in-depth: track the per-turn `stream_mode` and reject any disallowed `LocalToolRequest` before local execution, returning a terminal tool error through `tools.result`.
- Mode is runtime CLI state and is passed per turn. It is not persisted to Postgres, but it persists inside the running TUI across consecutive prompts.
- `Shift+Tab` toggles modes only on Home/Chat input when no chat stream is active. Do not steal `Tab` or Tool Editor `Shift+Tab` focus behavior.
- Home/Chat textareas show the primary `[ MODE: ... ]` indicator; other route footers show a compact mode indicator.
- Persist assistant turns as a side-channel from sanitized stream events. Never block the SSE stream on DB writes.

## Tool Runtime
- Tier-1 tools are compiled Rust tools in `agentic_tools::registry()`. Adding or editing them requires code changes, rebuild, and restart.
- Tier-2 tools are Python or shell scripts stored as versioned rows in Postgres. They are added, edited, tested, and activated at runtime without rebuild or server restart.
- The server merges both tiers in `tools.list`; tier is an implementation detail except where mode policy explicitly filters runtime access.
- Tier-2 script bodies travel inline in `LocalToolRequest.script`; the server stays free of script execution. Never persist Tier-2 script bodies in UI history.
- Dynamic Rig integration for Tier-2 uses `DynamicProxyTool` with an overridden `name()` because Rig keys its `ToolSet` by `name()`, not `NAME`.
- Rig tool loops need an explicit multi-turn budget (`AGENT_MAX_TOOL_TURNS`) on both the agent and streaming prompt request.

## Local Tool Safety
- The CLI workspace root is `std::env::current_dir().canonicalize()` at startup.
- All local path tools must go through `WorkspaceGuard`; block absolute paths, `..`, symlink escapes, `.git`, `target`, `.env*`, `.pem`, `.key`, and `.agent-tools/`.
- Read-only tools run automatically. Mutating tools and any non-`ReadOnly` Tier-2 tool require inline CLI `Y/N` approval.
- Tier-2 scripts run in `<workspace>/.agent-tools/scratch/<uuid>/` via subprocess, with scrubbed env, cwd/TMPDIR isolation, output caps, timeout, and child kill-on-timeout.
- Do not add `run_command` or recursive deletion without a separate design pass for process control, timeouts, and stronger approval.

## Tool Author And Editor
- `POST /author/stream` drives LLM-authored Tier-2 tool creation; it never writes to `sessions` or `messages`.
- The author agent has exactly `set_draft`, `sandbox_run`, and `submit_tool`. It cannot recursively call the user-facing tools registry.
- `submit_tool` only writes a draft. Tools are not callable by chat until the user publishes them from `/tools`.
- `/tools` loads `tools.list` and `tools.management` in parallel and merges them in `App::set_tools_from_server(...)`.
- Keep `/tools` rendering in `apps/cli/src/ui/tools.rs`; keep route/input/state handling in `app.rs`; keep editor action dispatch in `tui.rs`.
- Agent-created tools require two approvals before edits: conceptual proposal, then implementation review listing exact files to modify/create.

## Text Areas And Cursors
- `apps/cli/src/ui/mod.rs` exposes `caret_xy(text, width)` and `wrap_chars(text, width)`. They must agree on coordinates.
- Never call `Paragraph::wrap(...)` on a focused textarea; pre-wrap with `wrap_chars` so the caret does not drift from rendered text.
- Read-only panes may use `Wrap { trim: false }` because they have no caret.

## Persistence
- Sessions and messages live in Postgres. `messages.parts` is `JSONB` storing `Vec<UiPart>`.
- Tier-2 tools live in `tools` and `tool_versions`; drafts persist across CLI restarts and surface in `/tools` until published or deleted.
- `tool_repo::delete_tool` cascades inside one transaction. Use it for full removal; use `delete_version` only for surgical history edits.
- Schema sync runs on server boot in `agentic_db::connect()` using SeaORM `create_table_from_entity(...).if_not_exists()`. No migration crate yet.
- SeaORM is pinned to `=2.0.0-rc.38` until stable 2.0 lands.

## UX Notes
- Routes: `/home`, `/sessions`, `/tools`; unknown routes render Not Found.
- Enter submits in chat; `Ctrl+Enter` / `Alt+Enter` insert newlines. In the Tool Editor's Description and Script fields, Enter inserts a newline.
- On `/tools` with editor closed: `N` opens the editor, Enter reopens the focused draft, `D` / Delete opens delete confirmation, `↑↓` navigates.
- In the Tool Editor: `Tab` / `Shift+Tab` cycle fields, `F2` toggles Python/Shell, `Ctrl+G` generates, `Ctrl+R` runs, `Ctrl+S` saves, `Ctrl+P` publishes, Esc returns to the tools list.
- Esc on Sessions and Tools returns Home; in the Tool Editor or delete confirmation, Esc returns to the previous screen; elsewhere Esc quits unless inline input/approval consumes it.
- Mouse capture is intentionally off so terminal-native selection and copy work.

## Commands
- Server loads `apps/server/.env`; `DEEPSEEK_API_KEY` and `DATABASE_URL` are required for normal chat operation.
- CLI uses `AGENTS_SERVER_URL`, defaulting to `http://127.0.0.1:3000`.
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server`
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period, e.g. `Build Hybrid Coding Agent Tool Runtime`.
