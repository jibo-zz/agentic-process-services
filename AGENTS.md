# AGENTS.md

## Repo Shape
- Rust 2024 Cargo workspace with resolver `3`; root `Cargo.toml` owns members and shared dependency versions.
- Runnable apps live in `apps/`: `apps/server` is the Axum service, `apps/cli` is the Ratatui/Crossterm TUI.
- Shared crates live in `crates/`: protocol DTOs, DB persistence, core/config, and `agentic-tools` for tool contracts, instructions, runners, sandbox runtime, and guardrails.
- Do not couple `apps/server` and `apps/cli` directly. Move shared contracts or behavior into `crates/`.

## Skills
- Use `cargo-rust` for dependency, check, test, format, lint, and workspace changes.
- Use `ratatui-tui` for CLI layout, input handling, hotkeys, textareas, async event loops, and rendering changes.
- Use `rig` for server agent construction, preambles, tool registration, streaming, and tool-loop behavior.
- Use `axum` for server routing/extractors/middleware and `sea-orm` for DB entities/repos/schema sync.

## App Boundaries
- Keep entrypoints thin; `main.rs` installs runtime/error/panic handling and delegates.
- In `apps/cli`: `app.rs` owns state/input, `tui.rs` owns the event loop and async task spawning, `ui/<screen>.rs` owns rendering.
- `apps/cli/src/client.rs` exposes typed wrappers. Raw JSON-RPC stays private behind `rpc_call<T>()`.
- The CLI must not depend on Rig, Open-Meteo, or provider-specific tool types. It executes generic local requests and renders protocol events.

## Chat And Modes
- Chat uses `POST /chat/stream` SSE. JSON-RPC remains for non-streaming calls on `POST /rpc`.
- Server loads prior turns from Postgres using `ChatRequest { session_id, message, mode }`; clients do not send chat history.
- `AgentMode` lives in `crates/protocol`. Add modes by extending `AgentMode::ALL` and policy helpers, not by hardcoding a two-mode toggle.
- `BUILD` is default and allows the full tool set subject to approval rules.
- `PLAN` is read-only: only Tier-1 `list_files`, `read_file`, and `search_files` are allowed. It blocks mutating tools, weather/network tools, all Tier-2 subprocess tools, and sandbox execution.
- Mode instructions live in `crates/agent-tools/src/instructions.rs`; server chat assembly must use `coding_agent_preamble(mode)`.
- Defense in depth is required on both sides: server attaches/rejects tools by mode, and CLI rejects disallowed `LocalToolRequest`s before execution.
- Mode is runtime CLI state passed per turn. It is not persisted to Postgres.
- `Shift+Tab` toggles modes only on Home/Chat input when no chat stream is active. Do not steal normal `Tab` or Tool Editor `Shift+Tab` behavior.
- Persist assistant turns as a side-channel from sanitized stream events. Never block SSE streaming on DB writes.

## Tool Runtime
- Tier-1 tools are compiled Rust tools in `agentic_tools::registry()`. Editing them requires rebuild/restart.
- Tier-2 tools are Python or shell scripts stored as versioned Postgres rows. They can be drafted, tested, activated, and deleted at runtime.
- Server merges Tier-1 and active Tier-2 tools in `tools.list`; tier is an implementation detail except for mode policy.
- Tier-2 script bodies travel inline in `LocalToolRequest.script` for CLI execution. Never persist script bodies in UI history.
- Dynamic Rig integration for Tier-2 uses `DynamicProxyTool` with an overridden `name()` because Rig keys `ToolSet` by `name()`.
- Rig tool loops need explicit multi-turn budgets (`AGENT_MAX_TOOL_TURNS`) on both agent and streaming prompt request.

## Sandbox Runtime
- Tier-2 execution now goes through `agentic_tools::sandbox::SandboxManager` in the CLI runtime.
- Phase 1 is internal only: no `sandbox_start`, `sandbox_status`, `sandbox_logs`, `sandbox_cancel`, TUI route, DB persistence, Docker, Firecracker, or third-party agent integration yet.
- Supported workloads are only structured Python and shell scripts. Do not add arbitrary `run_command`.
- Jobs are short-lived, in-memory, and use strict timeouts. Current default remains `10_000ms` for Tier-2 scripts.
- Each run creates `.agent-tools/sandbox/<job_id>/` and records final status/result in memory.
- Output remains final-only for now, with existing stdout/stderr caps enforced by the low-level runner. Design future log streaming with cursors, but do not add it prematurely.
- Future subagents, OpenClaw, Hermes, Docker, or VM backends must be adapters behind the sandbox contract, with the super agent as controller and sandbox policy as enforcement.

## Local Safety
- CLI workspace root is `std::env::current_dir().canonicalize()` at startup.
- All path tools must go through `WorkspaceGuard`; block absolute paths, `..`, symlink escapes, `.git`, `target`, `.env*`, `.pem`, `.key`, and `.agent-tools/`.
- Read-only tools run automatically. Mutating tools and non-`ReadOnly` Tier-2 tools require inline CLI `Y/N` approval.
- Subprocess execution uses scrubbed env, cwd/TMPDIR isolation, output caps, timeout, and child kill-on-timeout.
- Do not add recursive deletion or arbitrary command execution without a separate design pass for process control, timeout, approval, and audit policy.

## Tool Author And Editor
- `POST /author/stream` drives LLM-authored Tier-2 tool creation; it never writes to `sessions` or `messages`.
- The author agent has exactly `set_draft`, `sandbox_run`, and `submit_tool`. It cannot recursively call the user-facing registry.
- `submit_tool` is rejected unless at least one `sandbox_run` has been attempted for the current draft.
- `submit_tool` only writes a draft. Tools are not callable by chat until published from `/tools`.
- `/tools` loads `tools.list` and `tools.management` in parallel and merges them in `App::set_tools_from_server(...)`.
- Keep `/tools` rendering in `apps/cli/src/ui/tools.rs`; keep route/input/state in `app.rs`; keep editor action dispatch in `tui.rs`.
- Agent-created tools require two approvals before edits: conceptual proposal, then implementation review listing exact files to modify/create.

## Persistence
- Sessions and messages live in Postgres. `messages.parts` is JSONB storing `Vec<UiPart>`.
- Tier-2 tools live in `tools` and `tool_versions`; drafts persist across CLI restarts until published or deleted.
- `tool_repo::delete_tool` cascades inside one transaction. Use it for full removal; use `delete_version` only for surgical history edits.
- Schema sync runs on server boot in `agentic_db::connect()` using `create_table_from_entity(...).if_not_exists()`. No migration crate yet.
- SeaORM is pinned to `=2.0.0-rc.38` until stable 2.0 lands.
- Sandbox jobs are not persisted yet.

## UX Notes
- Routes: `/home`, `/sessions`, `/tools`; unknown routes render Not Found.
- Enter submits in chat; `Ctrl+Enter` / `Alt+Enter` insert newlines. In Tool Editor Description/Script fields, Enter inserts a newline.
- On `/tools` with editor closed: `N` opens editor, Enter reopens focused draft, `D` / Delete opens delete confirmation, `↑↓` navigates.
- In Tool Editor: `Tab` / `Shift+Tab` cycle fields, `F2` toggles Python/Shell, `Ctrl+G` generates, `Ctrl+R` runs, `Ctrl+S` saves, `Ctrl+P` publishes, Esc returns to list.
- Esc on Sessions/Tools returns Home; in Tool Editor/delete confirmation it returns to previous screen; elsewhere Esc quits unless inline input/approval consumes it.
- Focused textareas must pre-wrap with `wrap_chars`; do not use `Paragraph::wrap(...)` where caret coordinates matter.
- Mouse capture is intentionally off so terminal-native selection/copy works.

## Commands
- Server loads `apps/server/.env`; `DEEPSEEK_API_KEY` and `DATABASE_URL` are required for normal chat.
- CLI uses `AGENTS_SERVER_URL`, defaulting to `http://127.0.0.1:3000`.
- Check: `cargo check --workspace`
- Test: `cargo test --workspace`
- Format: `cargo fmt --all` / `cargo fmt --all -- --check`
- Lint: `cargo clippy --workspace --all-targets`
- Run server: `cargo run -p server`
- Run TUI: `cargo run -p cli`

## Git
- Commit subjects: short, imperative, title-cased, no trailing period, e.g. `Build Hybrid Coding Agent Tool Runtime`.
