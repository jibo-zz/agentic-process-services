use agentic_protocol::{LocalToolScript, ToolScriptLanguage};
use agentic_tools::{
    WorkspaceGuard, execute_local_tool,
    sandbox::{SandboxManager, SandboxRunKind, SandboxRunRequest},
};
use serde_json::Value;
use std::time::Duration;

pub fn execute(guard: &WorkspaceGuard, name: &str, input: Value) -> Result<Value, String> {
    execute_local_tool(guard, name, input).map_err(|error| error.to_string())
}

/// Runs a Tier-2 (DB-authored) script through the sandbox manager and returns a summary object.
pub async fn execute_tier2(
    _guard: &WorkspaceGuard,
    sandbox_manager: &SandboxManager,
    input: Value,
    script: LocalToolScript,
) -> Result<Value, String> {
    let args_json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_owned());
    let kind = match script.language {
        ToolScriptLanguage::Python => SandboxRunKind::PythonScript,
        ToolScriptLanguage::Shell => SandboxRunKind::ShellScript,
    };
    let timeout_ms = script.timeout_ms.max(100);
    let request = SandboxRunRequest {
        kind,
        script: script.script,
        args_json,
        timeout: Duration::from_millis(timeout_ms),
    };

    let outcome = sandbox_manager
        .run_script(request)
        .await
        .map_err(|error| error.to_string())?;

    if outcome.timed_out {
        return Err(format!("Script timed out after {}ms", outcome.duration_ms));
    }

    let exit_code = outcome.exit_code.unwrap_or(-1);
    if exit_code != 0 {
        let stderr = if outcome.stderr.is_empty() {
            outcome.stdout.clone()
        } else {
            outcome.stderr.clone()
        };
        return Err(format!("Script exited with code {exit_code}: {stderr}"));
    }

    // Try to surface a structured JSON payload; fall back to plain text in `stdout`.
    let parsed: Value = serde_json::from_str(&outcome.stdout)
        .unwrap_or_else(|_| Value::String(outcome.stdout.clone()));
    let summary = format!("Script exit {exit_code} in {}ms", outcome.duration_ms);
    Ok(serde_json::json!({
        "job_id": outcome.job_id,
        "output": parsed,
        "stdout": outcome.stdout,
        "stderr": outcome.stderr,
        "exit_code": exit_code,
        "duration_ms": outcome.duration_ms,
        "stdout_truncated": outcome.stdout_truncated,
        "stderr_truncated": outcome.stderr_truncated,
        "summary": summary,
    }))
}
