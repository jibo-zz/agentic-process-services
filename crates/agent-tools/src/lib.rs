pub mod instructions;
pub mod runner;

use agentic_protocol::{ToolDescriptor, ToolExecutionKind, ToolOutputPolicy, ToolRisk};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    error::Error,
    fmt, fs, io,
    path::{Component, Path, PathBuf},
};

pub const LIST_FILES: &str = "list_files";
pub const READ_FILE: &str = "read_file";
pub const SEARCH_FILES: &str = "search_files";
pub const WRITE_FILE: &str = "write_file";
pub const EDIT_FILE: &str = "edit_file";
pub const DELETE_FILE: &str = "delete_file";
pub const DELETE_DIRECTORY: &str = "delete_directory";
pub const GET_CURRENT_WEATHER: &str = "get_current_weather";

const MAX_READ_BYTES: usize = 256 * 1024;
const MAX_WRITE_BYTES: usize = 512 * 1024;
const DEFAULT_MAX_RESULTS: usize = 200;
const MAX_SEARCH_FILE_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ToolExecution {
    ServerNative,
    LocalProxy { approval_required: bool },
}

#[derive(Debug, Clone, Copy)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: fn() -> Value,
    pub execution: ToolExecution,
    pub risk: ToolRisk,
    pub output_policy: ToolOutputPolicy,
}

pub fn registry() -> &'static [ToolSpec] {
    &[
        ToolSpec {
            name: GET_CURRENT_WEATHER,
            description: "Get the current weather for a city. Use this for current weather questions.",
            parameters: weather_schema,
            execution: ToolExecution::ServerNative,
            risk: ToolRisk::Network,
            output_policy: ToolOutputPolicy::FullAllowed,
        },
        ToolSpec {
            name: LIST_FILES,
            description: "List files under a workspace-relative directory. Use this to inspect project structure.",
            parameters: list_files_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: false,
            },
            risk: ToolRisk::ReadOnly,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: READ_FILE,
            description: "Read a UTF-8 text file by workspace-relative path. Use this before editing existing files.",
            parameters: read_file_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: false,
            },
            risk: ToolRisk::ReadOnly,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: SEARCH_FILES,
            description: "Search workspace text files with a plain substring query.",
            parameters: search_files_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: false,
            },
            risk: ToolRisk::ReadOnly,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: WRITE_FILE,
            description: "Create or overwrite a UTF-8 text file by workspace-relative path. Requires user approval.",
            parameters: write_file_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: true,
            },
            risk: ToolRisk::WritesFiles,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: EDIT_FILE,
            description: "Edit a UTF-8 text file by exact string replacement. Requires user approval.",
            parameters: edit_file_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: true,
            },
            risk: ToolRisk::WritesFiles,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: DELETE_FILE,
            description: "Permanently delete a workspace-relative file. Requires user approval and refuses directories.",
            parameters: delete_file_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: true,
            },
            risk: ToolRisk::DeletesFiles,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
        ToolSpec {
            name: DELETE_DIRECTORY,
            description: "Delete an empty workspace-relative directory. Requires user approval, refuses files, refuses the workspace root, and does not delete recursively.",
            parameters: delete_directory_schema,
            execution: ToolExecution::LocalProxy {
                approval_required: true,
            },
            risk: ToolRisk::DeletesDirectories,
            output_policy: ToolOutputPolicy::FullToModelSummaryToUi,
        },
    ]
}

pub fn descriptors() -> Vec<ToolDescriptor> {
    registry()
        .iter()
        .map(|tool| {
            let (execution, approval_required) = match tool.execution {
                ToolExecution::ServerNative => (ToolExecutionKind::ServerNative, false),
                ToolExecution::LocalProxy { approval_required } => {
                    (ToolExecutionKind::LocalProxy, approval_required)
                }
            };
            ToolDescriptor {
                name: tool.name.to_owned(),
                description: tool.description.to_owned(),
                execution,
                approval_required,
                risk: tool.risk,
                output_policy: tool.output_policy,
                parameters: (tool.parameters)(),
            }
        })
        .collect()
}

pub fn spec(name: &str) -> Option<&'static ToolSpec> {
    registry().iter().find(|tool| tool.name == name)
}

pub fn local_tool_names() -> impl Iterator<Item = &'static str> {
    registry().iter().filter_map(|tool| match tool.execution {
        ToolExecution::LocalProxy { .. } => Some(tool.name),
        ToolExecution::ServerNative => None,
    })
}

#[derive(Debug, Clone)]
pub struct WorkspaceGuard {
    root: PathBuf,
}

impl WorkspaceGuard {
    pub fn from_current_dir() -> Result<Self, ToolError> {
        let root = std::env::current_dir()
            .map_err(|error| ToolError::io("read current directory", error))?
            .canonicalize()
            .map_err(|error| ToolError::io("canonicalize current directory", error))?;
        Ok(Self { root })
    }

    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| ToolError::io("canonicalize workspace root", error))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn resolve_existing_file(&self, path: &str) -> Result<GuardedPath, ToolError> {
        let candidate = self.resolve_existing(path)?;
        if !candidate.full_path.is_file() {
            return Err(ToolError::Denied(format!("'{path}' is not a file")));
        }
        Ok(candidate)
    }

    pub fn resolve_existing_dir(&self, path: &str) -> Result<GuardedPath, ToolError> {
        let candidate = self.resolve_existing(path)?;
        if !candidate.full_path.is_dir() {
            return Err(ToolError::Denied(format!("'{path}' is not a directory")));
        }
        Ok(candidate)
    }

    pub fn resolve_for_write(&self, path: &str) -> Result<GuardedPath, ToolError> {
        let rel_path = validate_relative_path(path)?;
        deny_sensitive_path(&rel_path)?;
        let parent_rel = rel_path.parent().unwrap_or_else(|| Path::new(""));
        let parent_full = self.root.join(parent_rel);
        let parent_canonical = if parent_full.exists() {
            parent_full
                .canonicalize()
                .map_err(|error| ToolError::io("canonicalize parent directory", error))?
        } else {
            let nearest = nearest_existing_parent(&self.root, parent_rel)?;
            nearest
                .canonicalize()
                .map_err(|error| ToolError::io("canonicalize nearest parent directory", error))?
        };
        if !parent_canonical.starts_with(&self.root) {
            return Err(ToolError::Denied("path escapes workspace root".to_owned()));
        }

        let full_path = self.root.join(&rel_path);
        if full_path.exists() {
            let canonical = full_path
                .canonicalize()
                .map_err(|error| ToolError::io("canonicalize target path", error))?;
            if !canonical.starts_with(&self.root) {
                return Err(ToolError::Denied("path escapes workspace root".to_owned()));
            }
        }

        Ok(GuardedPath {
            rel_path,
            full_path,
        })
    }

    fn resolve_existing(&self, path: &str) -> Result<GuardedPath, ToolError> {
        let rel_path = validate_relative_path(path)?;
        deny_sensitive_path(&rel_path)?;
        let full_path = self.root.join(&rel_path);
        let canonical = full_path
            .canonicalize()
            .map_err(|error| ToolError::io("canonicalize path", error))?;
        if !canonical.starts_with(&self.root) {
            return Err(ToolError::Denied("path escapes workspace root".to_owned()));
        }
        Ok(GuardedPath {
            rel_path,
            full_path: canonical,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GuardedPath {
    rel_path: PathBuf,
    full_path: PathBuf,
}

impl GuardedPath {
    pub fn relative_display(&self) -> String {
        path_to_slash_string(&self.rel_path)
    }

    pub fn full_path(&self) -> &Path {
        &self.full_path
    }
}

#[derive(Debug, Serialize)]
pub struct ToolErrorPayload {
    pub error: String,
}

#[derive(Debug)]
pub enum ToolError {
    Denied(String),
    InvalidInput(String),
    Io {
        context: &'static str,
        error: io::Error,
    },
    Json(serde_json::Error),
}

impl ToolError {
    fn io(context: &'static str, error: io::Error) -> Self {
        Self::Io { context, error }
    }

    pub fn payload(&self) -> ToolErrorPayload {
        ToolErrorPayload {
            error: self.to_string(),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Denied(message) | Self::InvalidInput(message) => f.write_str(message),
            Self::Io { context, error } => write!(f, "{context} failed: {error}"),
            Self::Json(error) => write!(f, "invalid tool input: {error}"),
        }
    }
}

impl Error for ToolError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { error, .. } => Some(error),
            Self::Json(error) => Some(error),
            Self::Denied(_) | Self::InvalidInput(_) => None,
        }
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ListFilesArgs {
    pub path: Option<String>,
    pub max_results: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ListFilesOutput {
    pub path: String,
    pub files: Vec<String>,
    pub truncated: bool,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ReadFileArgs {
    pub path: String,
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ReadFileOutput {
    pub path: String,
    pub content: String,
    pub bytes: usize,
    pub lines: usize,
    pub truncated: bool,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchFilesArgs {
    pub query: String,
    pub path: Option<String>,
    pub max_results: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct SearchFilesOutput {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub truncated: bool,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct WriteFileOutput {
    pub path: String,
    pub bytes: usize,
    pub overwritten: bool,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct EditFileArgs {
    pub path: String,
    pub old: String,
    pub new: String,
    pub replace_all: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct EditFileOutput {
    pub path: String,
    pub replacements: usize,
    pub bytes: usize,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteFileArgs {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteFileOutput {
    pub path: String,
    pub deleted: bool,
    pub summary: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct DeleteDirectoryArgs {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteDirectoryOutput {
    pub path: String,
    pub deleted: bool,
    pub summary: String,
}

pub fn execute_local_tool(
    guard: &WorkspaceGuard,
    name: &str,
    input: Value,
) -> Result<Value, ToolError> {
    match name {
        LIST_FILES => to_value(list_files(guard, serde_json::from_value(input)?)?),
        READ_FILE => to_value(read_file(guard, serde_json::from_value(input)?)?),
        SEARCH_FILES => to_value(search_files(guard, serde_json::from_value(input)?)?),
        WRITE_FILE => to_value(write_file(guard, serde_json::from_value(input)?)?),
        EDIT_FILE => to_value(edit_file(guard, serde_json::from_value(input)?)?),
        DELETE_FILE => to_value(delete_file(guard, serde_json::from_value(input)?)?),
        DELETE_DIRECTORY => to_value(delete_directory(guard, serde_json::from_value(input)?)?),
        _ => Err(ToolError::InvalidInput(format!(
            "unknown local tool '{name}'"
        ))),
    }
}

pub fn approval_required(name: &str) -> bool {
    spec(name).is_some_and(|tool| match tool.execution {
        ToolExecution::LocalProxy { approval_required } => approval_required,
        ToolExecution::ServerNative => false,
    })
}

pub fn approval_summary(name: &str, input: &Value, guard: &WorkspaceGuard) -> String {
    match name {
        WRITE_FILE => match serde_json::from_value::<WriteFileArgs>(input.clone()) {
            Ok(args) => match guard.resolve_for_write(&args.path) {
                Ok(path) if path.full_path().exists() => {
                    format!("Overwrite {}?", path.relative_display())
                }
                Ok(path) => format!("Write {}?", path.relative_display()),
                Err(_) => format!("Write {}?", args.path),
            },
            Err(_) => "Approve write_file?".to_owned(),
        },
        EDIT_FILE => match serde_json::from_value::<EditFileArgs>(input.clone()) {
            Ok(args) => format!("Edit {}?", args.path),
            Err(_) => "Approve edit_file?".to_owned(),
        },
        DELETE_FILE => match serde_json::from_value::<DeleteFileArgs>(input.clone()) {
            Ok(args) => format!("Delete {} permanently?", args.path),
            Err(_) => "Approve delete_file?".to_owned(),
        },
        DELETE_DIRECTORY => match serde_json::from_value::<DeleteDirectoryArgs>(input.clone()) {
            Ok(args) => format!("Delete empty directory {} permanently?", args.path),
            Err(_) => "Approve delete_directory?".to_owned(),
        },
        _ => format!("Approve {name}?"),
    }
}

pub fn sanitized_output(name: &str, value: &Value) -> Value {
    let summary = value
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("Tool completed");
    let path = value.get("path").and_then(Value::as_str);
    match path {
        Some(path) => serde_json::json!({ "summary": summary, "path": path, "tool": name }),
        None => serde_json::json!({ "summary": summary, "tool": name }),
    }
}

pub fn sanitized_error(name: &str, error: &str) -> Value {
    serde_json::json!({ "summary": error, "tool": name })
}

pub fn sanitized_tool_value(value: &Value) -> Value {
    let mut output = serde_json::Map::new();
    for key in [
        "summary",
        "path",
        "query",
        "bytes",
        "lines",
        "truncated",
        "overwritten",
        "replacements",
    ] {
        if let Some(value) = value.get(key) {
            output.insert(key.to_owned(), value.clone());
        }
    }
    if output.is_empty() {
        serde_json::json!({ "summary": "Tool event" })
    } else {
        Value::Object(output)
    }
}

pub fn stream_summary(name: &str, input: &Value) -> String {
    let path = input.get("path").and_then(Value::as_str).unwrap_or(".");
    match name {
        LIST_FILES => format!("List files under {path}"),
        READ_FILE => format!("Read {path}"),
        SEARCH_FILES => {
            let query = input.get("query").and_then(Value::as_str).unwrap_or("");
            format!("Search {path} for '{query}'")
        }
        WRITE_FILE => format!("Write {path}"),
        EDIT_FILE => format!("Edit {path}"),
        DELETE_FILE => format!("Delete {path}"),
        DELETE_DIRECTORY => format!("Delete directory {path}"),
        _ => format!("Run {name}"),
    }
}

fn list_files(guard: &WorkspaceGuard, args: ListFilesArgs) -> Result<ListFilesOutput, ToolError> {
    let input_path = args.path.unwrap_or_else(|| ".".to_owned());
    let dir = guard.resolve_existing_dir(&input_path)?;
    let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS).min(1000);
    let mut files = Vec::new();
    collect_files(dir.full_path(), guard.root(), max_results, &mut files)?;
    let truncated = files.len() >= max_results;
    files.sort();
    Ok(ListFilesOutput {
        path: dir.relative_display(),
        summary: format!("Listed {} files", files.len()),
        files,
        truncated,
    })
}

fn read_file(guard: &WorkspaceGuard, args: ReadFileArgs) -> Result<ReadFileOutput, ToolError> {
    let path = guard.resolve_existing_file(&args.path)?;
    let max_bytes = args.max_bytes.unwrap_or(MAX_READ_BYTES).min(MAX_READ_BYTES);
    let mut bytes =
        fs::read(path.full_path()).map_err(|error| ToolError::io("read file", error))?;
    let truncated = bytes.len() > max_bytes;
    if truncated {
        bytes.truncate(max_bytes);
    }
    let content = String::from_utf8(bytes)
        .map_err(|_| ToolError::InvalidInput(format!("'{}' is not valid UTF-8", args.path)))?;
    let bytes = content.len();
    let lines = content.lines().count();
    Ok(ReadFileOutput {
        path: path.relative_display(),
        content,
        bytes,
        lines,
        truncated,
        summary: format!("Read {lines} lines from {}", path.relative_display()),
    })
}

fn search_files(
    guard: &WorkspaceGuard,
    args: SearchFilesArgs,
) -> Result<SearchFilesOutput, ToolError> {
    if args.query.is_empty() {
        return Err(ToolError::InvalidInput(
            "query must not be empty".to_owned(),
        ));
    }
    let input_path = args.path.unwrap_or_else(|| ".".to_owned());
    let target = guard.resolve_existing(&input_path)?;
    let max_results = args.max_results.unwrap_or(DEFAULT_MAX_RESULTS).min(1000);
    let mut matches = Vec::new();
    if target.full_path().is_file() {
        search_file(
            target.full_path(),
            guard.root(),
            &args.query,
            max_results,
            &mut matches,
        )?;
    } else {
        let mut files = Vec::new();
        collect_files(target.full_path(), guard.root(), usize::MAX, &mut files)?;
        for file in files {
            if matches.len() >= max_results {
                break;
            }
            search_file(
                &guard.root().join(&file),
                guard.root(),
                &args.query,
                max_results,
                &mut matches,
            )?;
        }
    }
    let truncated = matches.len() >= max_results;
    Ok(SearchFilesOutput {
        summary: format!("Found {} matches for '{}'", matches.len(), args.query),
        query: args.query,
        matches,
        truncated,
    })
}

fn write_file(guard: &WorkspaceGuard, args: WriteFileArgs) -> Result<WriteFileOutput, ToolError> {
    if args.content.len() > MAX_WRITE_BYTES {
        return Err(ToolError::InvalidInput(format!(
            "content is too large; limit is {MAX_WRITE_BYTES} bytes"
        )));
    }
    let path = guard.resolve_for_write(&args.path)?;
    let overwritten = path.full_path().exists();
    if let Some(parent) = path.full_path().parent() {
        fs::create_dir_all(parent)
            .map_err(|error| ToolError::io("create parent directories", error))?;
    }
    fs::write(path.full_path(), args.content.as_bytes())
        .map_err(|error| ToolError::io("write file", error))?;
    let bytes = args.content.len();
    let action = if overwritten { "Updated" } else { "Created" };
    Ok(WriteFileOutput {
        path: path.relative_display(),
        bytes,
        overwritten,
        summary: format!("{action} {} ({bytes} bytes)", path.relative_display()),
    })
}

fn edit_file(guard: &WorkspaceGuard, args: EditFileArgs) -> Result<EditFileOutput, ToolError> {
    if args.old.is_empty() {
        return Err(ToolError::InvalidInput(
            "old text must not be empty".to_owned(),
        ));
    }
    let path = guard.resolve_existing_file(&args.path)?;
    let content =
        fs::read_to_string(path.full_path()).map_err(|error| ToolError::io("read file", error))?;
    let count = content.matches(&args.old).count();
    if args.replace_all.unwrap_or(false) {
        if count == 0 {
            return Err(ToolError::InvalidInput("old text was not found".to_owned()));
        }
        let updated = content.replace(&args.old, &args.new);
        fs::write(path.full_path(), updated.as_bytes())
            .map_err(|error| ToolError::io("write file", error))?;
        return Ok(EditFileOutput {
            path: path.relative_display(),
            replacements: count,
            bytes: updated.len(),
            summary: format!(
                "Updated {} with {count} replacements",
                path.relative_display()
            ),
        });
    }
    if count != 1 {
        return Err(ToolError::InvalidInput(format!(
            "expected exactly one match, found {count}"
        )));
    }
    let updated = content.replacen(&args.old, &args.new, 1);
    fs::write(path.full_path(), updated.as_bytes())
        .map_err(|error| ToolError::io("write file", error))?;
    Ok(EditFileOutput {
        path: path.relative_display(),
        replacements: 1,
        bytes: updated.len(),
        summary: format!("Updated {} with 1 replacement", path.relative_display()),
    })
}

fn delete_file(
    guard: &WorkspaceGuard,
    args: DeleteFileArgs,
) -> Result<DeleteFileOutput, ToolError> {
    let path = guard.resolve_existing_file(&args.path)?;
    let relative = path.relative_display();
    fs::remove_file(path.full_path()).map_err(|error| ToolError::io("delete file", error))?;
    Ok(DeleteFileOutput {
        path: relative.clone(),
        deleted: true,
        summary: format!("Deleted {relative}"),
    })
}

fn delete_directory(
    guard: &WorkspaceGuard,
    args: DeleteDirectoryArgs,
) -> Result<DeleteDirectoryOutput, ToolError> {
    let path = guard.resolve_existing_dir(&args.path)?;
    let relative = path.relative_display();
    if relative == "." {
        return Err(ToolError::Denied(
            "refusing to delete the workspace root".to_owned(),
        ));
    }
    fs::remove_dir(path.full_path()).map_err(|error| ToolError::io("delete directory", error))?;
    Ok(DeleteDirectoryOutput {
        path: relative.clone(),
        deleted: true,
        summary: format!("Deleted empty directory {relative}"),
    })
}

fn to_value<T: Serialize>(value: T) -> Result<Value, ToolError> {
    serde_json::to_value(value).map_err(ToolError::Json)
}

fn collect_files(
    dir: &Path,
    root: &Path,
    max_results: usize,
    files: &mut Vec<String>,
) -> Result<(), ToolError> {
    if files.len() >= max_results {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|error| ToolError::io("list directory", error))?;
    for entry in entries {
        if files.len() >= max_results {
            break;
        }
        let entry = entry.map_err(|error| ToolError::io("read directory entry", error))?;
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else {
            continue;
        };
        if deny_sensitive_path(rel).is_err() || is_ignored_component(rel) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            collect_files(&path, root, max_results, files)?;
        } else if metadata.is_file() {
            files.push(path_to_slash_string(rel));
        }
    }
    Ok(())
}

fn search_file(
    path: &Path,
    root: &Path,
    query: &str,
    max_results: usize,
    matches: &mut Vec<SearchMatch>,
) -> Result<(), ToolError> {
    if matches.len() >= max_results {
        return Ok(());
    }
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return Ok(()),
    };
    if metadata.len() as usize > MAX_SEARCH_FILE_BYTES {
        return Ok(());
    }
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(_) => return Ok(()),
    };
    let rel = path.strip_prefix(root).unwrap_or(path);
    for (index, line) in content.lines().enumerate() {
        if matches.len() >= max_results {
            break;
        }
        if line.contains(query) {
            matches.push(SearchMatch {
                path: path_to_slash_string(rel),
                line: index + 1,
                text: line.chars().take(240).collect(),
            });
        }
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<PathBuf, ToolError> {
    if path.trim().is_empty() {
        return Err(ToolError::InvalidInput("path must not be empty".to_owned()));
    }
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(ToolError::Denied(
            "absolute paths are not allowed".to_owned(),
        ));
    }
    let mut rel = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => rel.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(ToolError::Denied(
                    "parent directory paths are not allowed".to_owned(),
                ));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(ToolError::Denied(
                    "absolute paths are not allowed".to_owned(),
                ));
            }
        }
    }
    if rel.as_os_str().is_empty() {
        return Ok(PathBuf::from("."));
    }
    Ok(rel)
}

fn deny_sensitive_path(path: &Path) -> Result<(), ToolError> {
    for component in path.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        let name = part.to_string_lossy();
        if name == ".git" || name == "target" || name == ".agent-tools" {
            return Err(ToolError::Denied(format!(
                "'{name}' is not accessible to tools"
            )));
        }
        if name == ".env"
            || name.starts_with(".env.")
            || name.ends_with(".pem")
            || name.ends_with(".key")
        {
            return Err(ToolError::Denied(format!(
                "'{name}' is sensitive and cannot be accessed"
            )));
        }
    }
    Ok(())
}

fn is_ignored_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, Component::Normal(part) if {
            let name = part.to_string_lossy();
            name == ".git" || name == "target" || name == "node_modules" || name == ".agent-tools"
        })
    })
}

fn nearest_existing_parent(root: &Path, rel_parent: &Path) -> Result<PathBuf, ToolError> {
    let mut current = root.to_path_buf();
    for component in rel_parent.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        let candidate = current.join(part);
        if candidate.exists() {
            current = candidate;
        } else {
            break;
        }
    }
    Ok(current)
}

fn path_to_slash_string(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.is_empty() {
        ".".to_owned()
    } else {
        text
    }
}

fn weather_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "city": { "type": "string", "description": "City name, for example Berlin or San Francisco." },
            "country": { "type": "string", "description": "Optional country name or country code for disambiguation." },
            "unit": { "type": "string", "description": "Temperature unit. Defaults to celsius. Use fahrenheit only when explicitly requested.", "enum": ["celsius", "fahrenheit"] }
        },
        "required": ["city"]
    })
}

fn list_files_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative directory. Defaults to ." },
            "max_results": { "type": "integer", "description": "Maximum number of files to return." }
        }
    })
}

fn read_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative file path." },
            "max_bytes": { "type": "integer", "description": "Optional read limit in bytes." }
        },
        "required": ["path"]
    })
}

fn search_files_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Plain substring to search for." },
            "path": { "type": "string", "description": "Workspace-relative file or directory. Defaults to ." },
            "max_results": { "type": "integer", "description": "Maximum number of matches to return." }
        },
        "required": ["query"]
    })
}

fn write_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative file path." },
            "content": { "type": "string", "description": "Complete UTF-8 file content to write." }
        },
        "required": ["path", "content"]
    })
}

fn edit_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative file path." },
            "old": { "type": "string", "description": "Exact text to replace." },
            "new": { "type": "string", "description": "Replacement text." },
            "replace_all": { "type": "boolean", "description": "Replace all matches. Defaults to false and then requires exactly one match." }
        },
        "required": ["path", "old", "new"]
    })
}

fn delete_file_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative file path to permanently delete. Directories are refused." }
        },
        "required": ["path"]
    })
}

fn delete_directory_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Workspace-relative empty directory path to permanently delete. Files, non-empty directories, and the workspace root are refused." }
        },
        "required": ["path"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs as unix_fs;

    #[test]
    fn guard_rejects_absolute_and_parent_paths() {
        let dir = tempfile::tempdir().unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        assert!(guard.resolve_for_write("/tmp/file").is_err());
        assert!(guard.resolve_for_write("../file").is_err());
    }

    #[test]
    fn guard_rejects_sensitive_paths() {
        let dir = tempfile::tempdir().unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        assert!(guard.resolve_for_write(".env").is_err());
        assert!(guard.resolve_for_write(".git/config").is_err());
    }

    #[test]
    fn guard_rejects_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        unix_fs::symlink(outside.path(), dir.path().join("outside")).unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        assert!(guard.resolve_existing_file("outside/secret.txt").is_err());
    }

    #[test]
    fn edit_requires_single_match_by_default() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "one one").unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        let err = edit_file(
            &guard,
            EditFileArgs {
                path: "file.txt".to_owned(),
                old: "one".to_owned(),
                new: "two".to_owned(),
                replace_all: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("expected exactly one match"));
    }

    #[test]
    fn registry_has_unique_names_and_valid_schemas() {
        let mut names = std::collections::HashSet::new();
        for tool in registry() {
            assert!(
                names.insert(tool.name),
                "duplicate tool name: {}",
                tool.name
            );
            let schema = (tool.parameters)();
            assert_eq!(schema.get("type").and_then(Value::as_str), Some("object"));
        }
        for name in [
            LIST_FILES,
            READ_FILE,
            SEARCH_FILES,
            WRITE_FILE,
            EDIT_FILE,
            DELETE_FILE,
            DELETE_DIRECTORY,
        ] {
            assert!(local_tool_names().any(|tool_name| tool_name == name));
        }
    }

    #[test]
    fn read_file_returns_content_and_summary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "hello\nworld\n").unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        let output = read_file(
            &guard,
            ReadFileArgs {
                path: "file.txt".to_owned(),
                max_bytes: None,
            },
        )
        .unwrap();
        assert_eq!(output.content, "hello\nworld\n");
        assert_eq!(output.lines, 2);
        assert!(output.summary.contains("Read 2 lines"));
    }

    #[test]
    fn delete_file_removes_file_and_refuses_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.txt"), "delete me").unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();
        let output = delete_file(
            &guard,
            DeleteFileArgs {
                path: "file.txt".to_owned(),
            },
        )
        .unwrap();
        assert!(output.deleted);
        assert!(!dir.path().join("file.txt").exists());
        assert!(
            delete_file(
                &guard,
                DeleteFileArgs {
                    path: "nested".to_owned(),
                },
            )
            .is_err()
        );
    }

    #[test]
    fn delete_directory_removes_empty_directory_only() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("empty")).unwrap();
        fs::create_dir(dir.path().join("non_empty")).unwrap();
        fs::write(dir.path().join("non_empty/file.txt"), "content").unwrap();
        fs::write(dir.path().join("file.txt"), "content").unwrap();
        let guard = WorkspaceGuard::new(dir.path()).unwrap();

        let output = delete_directory(
            &guard,
            DeleteDirectoryArgs {
                path: "empty".to_owned(),
            },
        )
        .unwrap();
        assert!(output.deleted);
        assert!(!dir.path().join("empty").exists());
        assert!(
            delete_directory(
                &guard,
                DeleteDirectoryArgs {
                    path: "non_empty".to_owned(),
                },
            )
            .is_err()
        );
        assert!(
            delete_directory(
                &guard,
                DeleteDirectoryArgs {
                    path: "file.txt".to_owned(),
                },
            )
            .is_err()
        );
        assert!(
            delete_directory(
                &guard,
                DeleteDirectoryArgs {
                    path: ".".to_owned(),
                },
            )
            .is_err()
        );
    }
}
