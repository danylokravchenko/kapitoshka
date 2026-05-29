use rig::completion::ToolDefinition;
use rig::tool::Tool;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};

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
            description:
                "Read the contents of a file. Paths are relative to the working directory."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to read" }
                },
                "required": ["path"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);
        tracing::info!(path = %path.display(), "reading file");
        std::fs::read_to_string(&path).map_err(|e| FsError(format!("{}: {e}", path.display())))
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
}

impl WriteFile {
    pub fn new(working_dir: &str) -> Self {
        Self {
            working_dir: working_dir.to_string(),
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
            description: "Write content to a file, creating it (and any missing parent directories) if it doesn't exist. Overwrites existing content.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to write" },
                    "content": { "type": "string", "description": "Content to write" }
                },
                "required": ["path", "content"]
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let path = resolve(&self.working_dir, &args.path);
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
            description: "List the contents of a directory. Defaults to the working directory if no path is given.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to list (optional, defaults to working dir)" }
                }
            }),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        let dir = args.path.as_deref().unwrap_or(".");
        let path = resolve(&self.working_dir, dir);
        tracing::info!(path = %path.display(), "listing directory");

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
