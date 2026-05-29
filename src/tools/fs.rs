use crate::permission::PermissionHandler;
use crate::ui;
use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
#[error("fs error: {0}")]
pub struct FsError(String);

fn resolve(working_dir: &str, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        Path::new(working_dir).join(p)
    }
}

// ── ReadFile ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ReadFileArgs {
    path: String,
    /// First line to return (1-indexed, inclusive). Omit to start from the beginning.
    start_line: Option<usize>,
    /// Last line to return (1-indexed, inclusive). Omit to read to the end.
    end_line: Option<usize>,
}

pub struct ReadFile {
    working_dir: String,
}

impl ReadFile {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: working_dir.to_string(),
        }
    }
}

impl Tool for ReadFile {
    const NAME: &'static str = "read_file";
    type Error = FsError;
    type Args = ReadFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Read the contents of a file. Use start_line/end_line to read a \
                specific range and avoid flooding the context window with large files. \
                Line numbers are 1-indexed and inclusive. \
                Paths are relative to the working directory."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to return (1-indexed, inclusive)"
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to return (1-indexed, inclusive)"
                    }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);
        tracing::info!(path = %path.display(), start = ?args.start_line, end = ?args.end_line, "reading file");

        ui::print_tool_action("read_file", &args.path);

        let content = std::fs::read_to_string(&path)
            .map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        let start = args.start_line.unwrap_or(1).saturating_sub(1);
        let lines: Vec<&str> = content.lines().collect();
        let end = args.end_line.unwrap_or(lines.len()).min(lines.len());

        if start >= lines.len() {
            return Err(FsError(format!(
                "start_line {s} is beyond end of file ({total} lines)",
                s = start + 1,
                total = lines.len()
            )));
        }

        // Prefix each line with its 1-indexed line number so the model can
        // refer back to specific lines when calling patch_file or search_file.
        let result = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>4} | {line}", start + i + 1))
            .collect::<Vec<_>>()
            .join("\n");

        Ok(result)
    }
}

// ── WriteFile ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct WriteFileArgs {
    path: String,
    content: String,
}

pub struct WriteFile {
    working_dir: String,
    permission: PermissionHandler,
}

impl WriteFile {
    pub fn new(working_dir: &str, permission: PermissionHandler) -> Self {
        Self {
            working_dir: working_dir.to_string(),
            permission,
        }
    }
}

impl Tool for WriteFile {
    const NAME: &'static str = "write_file";
    type Error = FsError;
    type Args = WriteFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Write content to a file, creating it (and any missing parent \
                directories) if it doesn't exist. Overwrites the entire file. \
                Prefer patch_file for targeted edits to existing files."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to write" },
                    "content": { "type": "string", "description": "Full content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);

        let summary = format!("{} ({} bytes)", args.path, args.content.len());
        ui::print_tool_action("write_file", &summary);

        let handler = Arc::clone(&self.permission);
        let path_clone = args.path.clone();
        let allowed = tokio::task::spawn_blocking(move || {
            handler("write_file", &format!("overwrite {path_clone}"))
        })
        .await
        .map_err(|e| FsError(format!("permission task failed: {e}")))?;
        if !allowed {
            return Err(FsError("permission denied by user".to_string()));
        }

        tracing::info!(path = %path.display(), "writing file");

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| FsError(format!("create dirs {}: {e}", parent.display())))?;
        }

        std::fs::write(&path, &args.content)
            .map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        Ok(format!(
            "wrote {} bytes to {}",
            args.content.len(),
            path.display()
        ))
    }
}

// ── PatchFile ─────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PatchFileArgs {
    path: String,
    /// Exact string to find in the file. Must match exactly once.
    old_str: String,
    /// String to replace it with.
    new_str: String,
}

pub struct PatchFile {
    working_dir: String,
    permission: PermissionHandler,
}

impl PatchFile {
    pub fn new(working_dir: &str, permission: PermissionHandler) -> Self {
        Self {
            working_dir: working_dir.to_string(),
            permission,
        }
    }
}

impl Tool for PatchFile {
    const NAME: &'static str = "patch_file";
    type Error = FsError;
    type Args = PatchFileArgs;
    type Output = String;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Replace an exact string in a file with new content. \
                old_str must match exactly once — use enough surrounding context \
                (e.g. the full function signature) to make it unique. \
                Prefer this over write_file for targeted edits."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to patch"
                    },
                    "old_str": {
                        "type": "string",
                        "description": "Exact string to find (must appear exactly once)"
                    },
                    "new_str": {
                        "type": "string",
                        "description": "Replacement string"
                    }
                },
                "required": ["path", "old_str", "new_str"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);

        let preview: String = args
            .old_str
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(60)
            .collect();
        let summary = format!("{} — «{preview}…»", args.path);
        ui::print_tool_action("patch_file", &summary);

        let handler = Arc::clone(&self.permission);
        let path_clone = args.path.clone();
        let allowed = tokio::task::spawn_blocking(move || {
            handler("patch_file", &format!("edit {path_clone}"))
        })
        .await
        .map_err(|e| FsError(format!("permission task failed: {e}")))?;
        if !allowed {
            return Err(FsError("permission denied by user".to_string()));
        }

        tracing::info!(path = %path.display(), "patching file");

        let content = std::fs::read_to_string(&path)
            .map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        let count = content.matches(args.old_str.as_str()).count();
        if count == 0 {
            return Err(FsError(format!("old_str not found in {}", path.display())));
        }
        if count > 1 {
            return Err(FsError(format!(
                "old_str matched {count} times in {} — add more context to make it unique",
                path.display()
            )));
        }

        let patched = content.replacen(args.old_str.as_str(), args.new_str.as_str(), 1);
        std::fs::write(&path, &patched).map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        Ok(format!("patched {}", path.display()))
    }
}

// ── ListDir ───────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ListDirArgs {
    path: Option<String>,
}

#[derive(Serialize)]
pub struct DirEntry {
    name: String,
    kind: String,
}

pub struct ListDir {
    working_dir: String,
}

impl ListDir {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: working_dir.to_string(),
        }
    }
}

impl Tool for ListDir {
    const NAME: &'static str = "list_dir";
    type Error = FsError;
    type Args = ListDirArgs;
    type Output = Vec<DirEntry>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "List the contents of a directory. Defaults to the working directory \
                if no path is given."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path to list (optional, defaults to working dir)"
                    }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let dir = args.path.as_deref().unwrap_or(".");
        let path = resolve(&self.working_dir, dir);
        tracing::info!(path = %path.display(), "listing directory");

        ui::print_tool_action("list_dir", dir);

        let entries =
            std::fs::read_dir(&path).map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        let mut result = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| FsError(e.to_string()))?;
            let kind = if entry.path().is_dir() { "dir" } else { "file" };
            result.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                kind: kind.to_string(),
            });
        }
        result.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(result)
    }
}

// ── SearchFile ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchFileArgs {
    path: String,
    /// Substring or literal string to search for (case-sensitive).
    pattern: String,
}

#[derive(Serialize)]
pub struct SearchMatch {
    line_number: usize,
    line: String,
}

pub struct SearchFile {
    working_dir: String,
}

impl SearchFile {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: working_dir.to_string(),
        }
    }
}

impl Tool for SearchFile {
    const NAME: &'static str = "search_file";
    type Error = FsError;
    type Args = SearchFileArgs;
    type Output = Vec<SearchMatch>;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: Self::NAME.to_string(),
            description: "Search for a literal string pattern in a file and return all \
                matching lines with their line numbers. Case-sensitive. \
                Use this to locate functions, types, or identifiers before reading \
                or patching, so you only fetch the lines you need."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to search"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Literal string to search for (case-sensitive)"
                    }
                },
                "required": ["path", "pattern"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);
        tracing::info!(path = %path.display(), pattern = %args.pattern, "searching file");

        ui::print_tool_action("search_file", &format!("{} «{}»", args.path, args.pattern));

        let content = std::fs::read_to_string(&path)
            .map_err(|e| FsError(format!("{}: {e}", path.display())))?;

        let matches = content
            .lines()
            .enumerate()
            .filter(|(_, line)| line.contains(args.pattern.as_str()))
            .map(|(i, line)| SearchMatch {
                line_number: i + 1,
                line: line.to_string(),
            })
            .collect();

        Ok(matches)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn temp_file(dir: &TempDir, name: &str, content: &str) -> String {
        let path = dir.path().join(name);
        fs::write(&path, content).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn test_permission() -> PermissionHandler {
        Arc::new(|_tool: &str, _action: &str| true)
    }

    // ── read_file ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn read_file_full() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "a.txt", "line1\nline2\nline3\n");
        let tool = ReadFile::new(dir.path().to_str().unwrap());
        let out = tool
            .call(ReadFileArgs {
                path: "a.txt".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap();
        assert!(out.contains("line1"));
        assert!(out.contains("line3"));
    }

    #[tokio::test]
    async fn read_file_range() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "b.txt", "a\nb\nc\nd\ne\n");
        let tool = ReadFile::new(dir.path().to_str().unwrap());
        let out = tool
            .call(ReadFileArgs {
                path: "b.txt".into(),
                start_line: Some(2),
                end_line: Some(4),
            })
            .await
            .unwrap();
        assert!(!out.contains("| a"));
        assert!(out.contains("| b"));
        assert!(out.contains("| d"));
        assert!(!out.contains("| e"));
    }

    #[tokio::test]
    async fn read_file_line_numbers_prefixed() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "c.txt", "hello\nworld\n");
        let tool = ReadFile::new(dir.path().to_str().unwrap());
        let out = tool
            .call(ReadFileArgs {
                path: "c.txt".into(),
                start_line: None,
                end_line: None,
            })
            .await
            .unwrap();
        assert!(out.contains("   1 | hello"));
        assert!(out.contains("   2 | world"));
    }

    #[tokio::test]
    async fn read_file_start_beyond_eof_errors() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "d.txt", "only one line\n");
        let tool = ReadFile::new(dir.path().to_str().unwrap());
        let err = tool
            .call(ReadFileArgs {
                path: "d.txt".into(),
                start_line: Some(99),
                end_line: None,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("beyond end of file"));
    }

    // ── patch_file ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn patch_file_replaces_match() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "e.txt", "foo bar baz\n");
        let tool = PatchFile::new(dir.path().to_str().unwrap(), test_permission());
        tool.call(PatchFileArgs {
            path: "e.txt".into(),
            old_str: "bar".into(),
            new_str: "QUX".into(),
        })
        .await
        .unwrap();
        let result = fs::read_to_string(dir.path().join("e.txt")).unwrap();
        assert_eq!(result, "foo QUX baz\n");
    }

    #[tokio::test]
    async fn patch_file_errors_when_not_found() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "f.txt", "hello world\n");
        let tool = PatchFile::new(dir.path().to_str().unwrap(), test_permission());
        let err = tool
            .call(PatchFileArgs {
                path: "f.txt".into(),
                old_str: "missing".into(),
                new_str: "x".into(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[tokio::test]
    async fn patch_file_errors_on_ambiguous_match() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "g.txt", "foo foo foo\n");
        let tool = PatchFile::new(dir.path().to_str().unwrap(), test_permission());
        let err = tool
            .call(PatchFileArgs {
                path: "g.txt".into(),
                old_str: "foo".into(),
                new_str: "bar".into(),
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("matched 3 times"));
    }

    #[tokio::test]
    async fn patch_file_multiline() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "h.txt", "fn old() {\n    todo!()\n}\n");
        let tool = PatchFile::new(dir.path().to_str().unwrap(), test_permission());
        tool.call(PatchFileArgs {
            path: "h.txt".into(),
            old_str: "fn old() {\n    todo!()\n}".into(),
            new_str: "fn new() {\n    42\n}".into(),
        })
        .await
        .unwrap();
        let result = fs::read_to_string(dir.path().join("h.txt")).unwrap();
        assert!(result.contains("fn new()"));
        assert!(!result.contains("fn old()"));
    }

    // ── search_file ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_file_finds_matches() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "i.txt", "apple\nbanana\napricot\ncherry\n");
        let tool = SearchFile::new(dir.path().to_str().unwrap());
        let matches = tool
            .call(SearchFileArgs {
                path: "i.txt".into(),
                pattern: "ap".into(),
            })
            .await
            .unwrap();
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].line_number, 1);
        assert_eq!(matches[0].line, "apple");
        assert_eq!(matches[1].line_number, 3);
        assert_eq!(matches[1].line, "apricot");
    }

    #[tokio::test]
    async fn search_file_no_matches_returns_empty() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "j.txt", "hello\nworld\n");
        let tool = SearchFile::new(dir.path().to_str().unwrap());
        let matches = tool
            .call(SearchFileArgs {
                path: "j.txt".into(),
                pattern: "xyz".into(),
            })
            .await
            .unwrap();
        assert!(matches.is_empty());
    }

    #[tokio::test]
    async fn search_file_case_sensitive() {
        let dir = TempDir::new().unwrap();
        temp_file(&dir, "k.txt", "Hello\nhello\nHELLO\n");
        let tool = SearchFile::new(dir.path().to_str().unwrap());
        let matches = tool
            .call(SearchFileArgs {
                path: "k.txt".into(),
                pattern: "hello".into(),
            })
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line_number, 2);
    }
}
