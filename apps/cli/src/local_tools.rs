use agentic_protocol::{LocalToolScript, ToolScriptLanguage};
use agentic_tools::{
    WorkspaceGuard, execute_local_tool,
    runner::{RunRequest, ScriptLanguage, run},
};
use serde_json::Value;
use std::{fs, path::PathBuf, time::Duration};
use uuid::Uuid;

pub fn execute(guard: &WorkspaceGuard, name: &str, input: Value) -> Result<Value, String> {
    execute_local_tool(guard, name, input).map_err(|error| error.to_string())
}

/// Runs a Tier-2 (DB-authored) script in an isolated scratch directory under the workspace and
/// returns a summary object. The scratch directory is removed after the run completes.
pub async fn execute_tier2(
    guard: &WorkspaceGuard,
    input: Value,
    script: LocalToolScript,
) -> Result<Value, String> {
    let scratch_root = guard.root().join(".agent-tools").join("scratch");
    fs::create_dir_all(&scratch_root).map_err(|error| error.to_string())?;
    let cwd: PathBuf = scratch_root.join(Uuid::new_v4().to_string());
    fs::create_dir(&cwd).map_err(|error| error.to_string())?;

    let args_json = serde_json::to_string(&input).unwrap_or_else(|_| "{}".to_owned());
    let language = match script.language {
        ToolScriptLanguage::Python => ScriptLanguage::Python,
        ToolScriptLanguage::Shell => ScriptLanguage::Shell,
    };
    let timeout_ms = script.timeout_ms.max(100);
    let request = RunRequest {
        language,
        script: script.script,
        args_json,
        timeout: Duration::from_millis(timeout_ms),
        cwd: cwd.clone(),
    };

    let outcome = run(request).await.map_err(|error| error.to_string());
    let _ = fs::remove_dir_all(&cwd);
    let outcome = outcome?;

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
