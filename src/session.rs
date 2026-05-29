use anyhow::Result;
use chrono::Local;
use std::fs::{self, File};
use std::io::Write as IoWrite;
use std::path::PathBuf;

pub struct Session {
    pub path: PathBuf,
    file: File,
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
}
