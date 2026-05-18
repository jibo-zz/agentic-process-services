use std::{
    error::Error,
    ffi::OsString,
    fmt, io,
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};

pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
const STDOUT_CAP: usize = 1024 * 1024;
const STDERR_CAP: usize = 256 * 1024;
const READ_CHUNK: usize = 8 * 1024;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ScriptLanguage {
    Python,
    Shell,
}

impl ScriptLanguage {
    fn program_and_args(self) -> (&'static str, &'static [&'static str]) {
        match self {
            ScriptLanguage::Python => ("python3", &["-"]),
            ScriptLanguage::Shell => ("sh", &["-s"]),
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub language: ScriptLanguage,
    pub script: String,
    pub args_json: String,
    pub timeout: Duration,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunOutcome {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug)]
pub enum RunError {
    Spawn { program: String, error: io::Error },
    Io(io::Error),
    CwdMissing(PathBuf),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Spawn { program, error } => {
                write!(f, "failed to spawn '{program}': {error}")
            }
            RunError::Io(error) => write!(f, "io error: {error}"),
            RunError::CwdMissing(path) => {
                write!(f, "scratch cwd missing: {}", path.display())
            }
        }
    }
}

impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Spawn { error, .. } | RunError::Io(error) => Some(error),
            RunError::CwdMissing(_) => None,
        }
    }
}

pub async fn run(req: RunRequest) -> Result<RunOutcome, RunError> {
    if !req.cwd.is_dir() {
        return Err(RunError::CwdMissing(req.cwd.clone()));
    }

    let started = Instant::now();
    let (program, args) = req.language.program_and_args();

    let path_env = std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"));

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(&req.cwd)
        .env_clear()
        .env("PATH", &path_env)
        .env("LANG", "C.UTF-8")
        .env("TMPDIR", &req.cwd)
        .env("ARGS_JSON", &req.args_json)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|error| RunError::Spawn {
        program: program.to_owned(),
        error,
    })?;

    let stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let script = req.script.clone();
    let stdin_task = tokio::spawn(async move {
        if let Some(mut stdin) = stdin {
            let _ = stdin.write_all(script.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
    });

    let stdout_task = tokio::spawn(read_capped(stdout, STDOUT_CAP));
    let stderr_task = tokio::spawn(read_capped(stderr, STDERR_CAP));

    let wait = tokio::time::timeout(req.timeout, child.wait()).await;

    let (exit_code, timed_out) = match wait {
        Ok(Ok(status)) => (status.code(), false),
        Ok(Err(_)) => (None, false),
        Err(_) => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            (None, true)
        }
    };

    let _ = stdin_task.await;
    let (stdout_buf, stdout_truncated) = stdout_task.await.unwrap_or_default();
    let (stderr_buf, stderr_truncated) = stderr_task.await.unwrap_or_default();

    Ok(RunOutcome {
        stdout: String::from_utf8_lossy(&stdout_buf).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_buf).into_owned(),
        exit_code,
        duration_ms: started.elapsed().as_millis() as u64,
        timed_out,
        stdout_truncated,
        stderr_truncated,
    })
}

async fn read_capped<R: AsyncReadExt + Unpin>(mut reader: R, cap: usize) -> (Vec<u8>, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; READ_CHUNK];
    let mut truncated = false;
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() >= cap {
                    truncated = true;
                    continue;
                }
                let remaining = cap - buf.len();
                let take = n.min(remaining);
                buf.extend_from_slice(&chunk[..take]);
                if take < n {
                    truncated = true;
                }
            }
            Err(_) => break,
        }
    }
    (buf, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(language: ScriptLanguage, script: &str, cwd: PathBuf, timeout_ms: u64) -> RunRequest {
        RunRequest {
            language,
            script: script.to_owned(),
            args_json: "{}".to_owned(),
            timeout: Duration::from_millis(timeout_ms),
            cwd,
        }
    }

    #[tokio::test]
    async fn python_prints_to_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run(req(
            ScriptLanguage::Python,
            "print('hi')",
            dir.path().to_path_buf(),
            5_000,
        ))
        .await
        .expect("run ok");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("hi"));
        assert!(!outcome.timed_out);
    }

    #[tokio::test]
    async fn python_reads_args_json_env() {
        let dir = tempfile::tempdir().unwrap();
        let script = r#"
import json, os
a = json.loads(os.environ["ARGS_JSON"])
print(json.dumps({"sum": a["x"] + a["y"]}))
"#;
        let mut request = req(
            ScriptLanguage::Python,
            script,
            dir.path().to_path_buf(),
            5_000,
        );
        request.args_json = r#"{"x":2,"y":3}"#.to_owned();
        let outcome = run(request).await.expect("run ok");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout.contains("\"sum\": 5"));
    }

    #[tokio::test]
    async fn shell_returns_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run(req(
            ScriptLanguage::Shell,
            "echo hi; exit 3",
            dir.path().to_path_buf(),
            5_000,
        ))
        .await
        .expect("run ok");
        assert_eq!(outcome.exit_code, Some(3));
        assert_eq!(outcome.stdout.trim_end(), "hi");
        assert!(!outcome.timed_out);
    }

    #[tokio::test]
    async fn infinite_loop_times_out_and_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = run(req(
            ScriptLanguage::Python,
            "while True: pass",
            dir.path().to_path_buf(),
            250,
        ))
        .await
        .expect("run ok");
        assert!(outcome.timed_out, "expected timed_out=true");
        assert_eq!(outcome.exit_code, None);
        assert!(outcome.duration_ms >= 200);
    }

    #[tokio::test]
    async fn large_stdout_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        // 2 MiB of 'x' on a single line — exceeds the 1 MiB cap.
        let script = "import sys\nsys.stdout.write('x' * (2 * 1024 * 1024))\n";
        let outcome = run(req(
            ScriptLanguage::Python,
            script,
            dir.path().to_path_buf(),
            10_000,
        ))
        .await
        .expect("run ok");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.stdout_truncated, "expected stdout_truncated=true");
        assert_eq!(outcome.stdout.len(), STDOUT_CAP);
    }

    #[tokio::test]
    async fn missing_cwd_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let err = run(req(ScriptLanguage::Shell, "true", missing, 1_000))
            .await
            .unwrap_err();
        assert!(matches!(err, RunError::CwdMissing(_)));
    }
}
