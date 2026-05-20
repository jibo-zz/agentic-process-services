use crate::runner::{RunOutcome, RunRequest, ScriptLanguage, run};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    error::Error,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SandboxRunKind {
    PythonScript,
    ShellScript,
}

impl SandboxRunKind {
    fn language(self) -> ScriptLanguage {
        match self {
            Self::PythonScript => ScriptLanguage::Python,
            Self::ShellScript => ScriptLanguage::Shell,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxRunRequest {
    pub kind: SandboxRunKind,
    pub script: String,
    pub args_json: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxRunResult {
    pub job_id: String,
    pub status: SandboxStatus,
    pub cwd: PathBuf,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub timed_out: bool,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SandboxJobSnapshot {
    pub job_id: String,
    pub kind: SandboxRunKind,
    pub status: SandboxStatus,
    pub cwd: PathBuf,
    pub started_at_secs: Option<u64>,
    pub finished_at_secs: Option<u64>,
    pub result: Option<SandboxRunResult>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
struct SandboxJob {
    job_id: String,
    kind: SandboxRunKind,
    status: SandboxStatus,
    cwd: PathBuf,
    started_at_secs: Option<u64>,
    finished_at_secs: Option<u64>,
    result: Option<SandboxRunResult>,
    error: Option<String>,
}

impl SandboxJob {
    fn snapshot(&self) -> SandboxJobSnapshot {
        SandboxJobSnapshot {
            job_id: self.job_id.clone(),
            kind: self.kind,
            status: self.status,
            cwd: self.cwd.clone(),
            started_at_secs: self.started_at_secs,
            finished_at_secs: self.finished_at_secs,
            result: self.result.clone(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SandboxManager {
    root: PathBuf,
    jobs: Arc<Mutex<HashMap<String, SandboxJob>>>,
}

impl SandboxManager {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            root: workspace_root.as_ref().join(".agent-tools").join("sandbox"),
            jobs: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn run_script(
        &self,
        request: SandboxRunRequest,
    ) -> Result<SandboxRunResult, SandboxError> {
        fs::create_dir_all(&self.root).map_err(SandboxError::CreateRoot)?;

        let job_id = Uuid::new_v4().to_string();
        let cwd = self.root.join(&job_id);
        fs::create_dir(&cwd).map_err(|error| SandboxError::CreateJobDir {
            path: cwd.clone(),
            error,
        })?;

        let job = SandboxJob {
            job_id: job_id.clone(),
            kind: request.kind,
            status: SandboxStatus::Queued,
            cwd: cwd.clone(),
            started_at_secs: None,
            finished_at_secs: None,
            result: None,
            error: None,
        };
        self.jobs.lock().await.insert(job_id.clone(), job);

        self.update_job(&job_id, |job| {
            job.status = SandboxStatus::Running;
            job.started_at_secs = Some(now_secs());
        })
        .await;

        let outcome = run(RunRequest {
            language: request.kind.language(),
            script: request.script,
            args_json: request.args_json,
            timeout: request.timeout,
            cwd: cwd.clone(),
        })
        .await;

        match outcome {
            Ok(outcome) => {
                let status = status_from_outcome(&outcome);
                let result = SandboxRunResult {
                    job_id: job_id.clone(),
                    status,
                    cwd,
                    stdout: outcome.stdout,
                    stderr: outcome.stderr,
                    exit_code: outcome.exit_code,
                    duration_ms: outcome.duration_ms,
                    timed_out: outcome.timed_out,
                    stdout_truncated: outcome.stdout_truncated,
                    stderr_truncated: outcome.stderr_truncated,
                };
                self.update_job(&job_id, |job| {
                    job.status = status;
                    job.finished_at_secs = Some(now_secs());
                    job.result = Some(result.clone());
                })
                .await;
                Ok(result)
            }
            Err(error) => {
                let message = error.to_string();
                self.update_job(&job_id, |job| {
                    job.status = SandboxStatus::Failed;
                    job.finished_at_secs = Some(now_secs());
                    job.error = Some(message.clone());
                })
                .await;
                Err(SandboxError::Run(message))
            }
        }
    }

    pub async fn snapshot(&self, job_id: &str) -> Option<SandboxJobSnapshot> {
        self.jobs.lock().await.get(job_id).map(SandboxJob::snapshot)
    }

    async fn update_job(&self, job_id: &str, update: impl FnOnce(&mut SandboxJob)) {
        if let Some(job) = self.jobs.lock().await.get_mut(job_id) {
            update(job);
        }
    }
}

#[derive(Debug)]
pub enum SandboxError {
    CreateRoot(io::Error),
    CreateJobDir { path: PathBuf, error: io::Error },
    Run(String),
}

impl fmt::Display for SandboxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CreateRoot(error) => write!(f, "create sandbox root failed: {error}"),
            Self::CreateJobDir { path, error } => {
                write!(
                    f,
                    "create sandbox job dir '{}' failed: {error}",
                    path.display()
                )
            }
            Self::Run(error) => f.write_str(error),
        }
    }
}

impl Error for SandboxError {}

fn status_from_outcome(outcome: &RunOutcome) -> SandboxStatus {
    if outcome.timed_out {
        SandboxStatus::TimedOut
    } else if outcome.exit_code == Some(0) {
        SandboxStatus::Succeeded
    } else {
        SandboxStatus::Failed
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(kind: SandboxRunKind, script: &str, timeout_ms: u64) -> SandboxRunRequest {
        SandboxRunRequest {
            kind,
            script: script.to_owned(),
            args_json: "{}".to_owned(),
            timeout: Duration::from_millis(timeout_ms),
        }
    }

    #[tokio::test]
    async fn python_script_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SandboxManager::new(dir.path());
        let result = manager
            .run_script(request(SandboxRunKind::PythonScript, "print('hi')", 5_000))
            .await
            .expect("script runs");

        assert_eq!(result.status, SandboxStatus::Succeeded);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("hi"));
    }

    #[tokio::test]
    async fn shell_script_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SandboxManager::new(dir.path());
        let result = manager
            .run_script(request(SandboxRunKind::ShellScript, "echo hi", 5_000))
            .await
            .expect("script runs");

        assert_eq!(result.status, SandboxStatus::Succeeded);
        assert_eq!(result.stdout.trim_end(), "hi");
    }

    #[tokio::test]
    async fn timeout_becomes_timed_out() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SandboxManager::new(dir.path());
        let result = manager
            .run_script(request(
                SandboxRunKind::PythonScript,
                "while True: pass",
                250,
            ))
            .await
            .expect("runner returns timeout outcome");

        assert_eq!(result.status, SandboxStatus::TimedOut);
        assert!(result.timed_out);
    }

    #[tokio::test]
    async fn completed_job_snapshot_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SandboxManager::new(dir.path());
        let result = manager
            .run_script(request(SandboxRunKind::PythonScript, "print('hi')", 5_000))
            .await
            .expect("script runs");
        let snapshot = manager.snapshot(&result.job_id).await.expect("snapshot");

        assert_eq!(snapshot.status, SandboxStatus::Succeeded);
        assert!(snapshot.started_at_secs.is_some());
        assert!(snapshot.finished_at_secs.is_some());
        assert!(snapshot.result.is_some());
    }

    #[tokio::test]
    async fn working_directory_uses_job_id() {
        let dir = tempfile::tempdir().unwrap();
        let manager = SandboxManager::new(dir.path());
        let result = manager
            .run_script(request(SandboxRunKind::PythonScript, "print('hi')", 5_000))
            .await
            .expect("script runs");

        assert_eq!(result.cwd, manager.root().join(&result.job_id));
        assert!(result.cwd.is_dir());
    }
}
