use anyhow::Result;
use chrono::Local;
use rig::completion::Message;
use std::fs::{self, File};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

pub struct Session {
    pub path: PathBuf,
    file: File,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SessionState {
    history: Vec<Message>,
    scratchpad: String,
}

impl Session {
    pub fn new(dir: &str, model: &str) -> Result<Self> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let sessions_dir = PathBuf::from(home).join(".kapitoshka").join("sessions");
        fs::create_dir_all(&sessions_dir)?;

        let now = Local::now();
        let filename = format!("{}.md", now.format("%Y-%m-%d-%H%M%S"));
        let path = sessions_dir.join(filename);

        let mut file = File::create(&path)?;
        writeln!(file, "# Session {}", now.format("%Y-%m-%d %H:%M:%S"))?;
        writeln!(file, "model: {model}  dir: {dir}\n")?;

        Ok(Self { path, file })
    }

    pub fn log_user(&mut self, msg: &str) -> Result<()> {
        writeln!(self.file, "## User\n\n{msg}\n")?;
        Ok(())
    }

    pub fn log_agent(&mut self, msg: &str) -> Result<()> {
        writeln!(self.file, "## Assistant\n\n{msg}\n\n---\n")?;
        Ok(())
    }

    /// Write conversation state to a sidecar JSON file next to the session log.
    /// Called after every successful turn so a crash loses at most one turn.
    pub fn save_state(&self, history: &[Message], scratchpad: &str) -> Result<()> {
        let state = SessionState {
            history: history.to_vec(),
            scratchpad: scratchpad.to_owned(),
        };
        let json = serde_json::to_string(&state)?;
        fs::write(self.state_path(), json)?;
        Ok(())
    }

    /// Load history and scratchpad from a previously saved state file.
    pub fn load_state(path: &Path) -> Result<(Vec<Message>, String)> {
        let json = fs::read_to_string(path)?;
        let state: SessionState = serde_json::from_str(&json)?;
        Ok((state.history, state.scratchpad))
    }

    fn state_path(&self) -> PathBuf {
        self.path.with_extension("json")
    }
}
