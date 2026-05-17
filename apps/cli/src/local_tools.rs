use agentic_tools::{WorkspaceGuard, execute_local_tool};
use serde_json::Value;

pub fn execute(guard: &WorkspaceGuard, name: &str, input: Value) -> Result<Value, String> {
    execute_local_tool(guard, name, input).map_err(|error| error.to_string())
}
