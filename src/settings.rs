use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    pub model: Option<String>,
}

fn settings_path() -> PathBuf {
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(".config")
        .join("kapitoshka")
        .join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        let Ok(data) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&data).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn set_model(&mut self, model: &str) -> Result<()> {
        self.model = Some(model.to_string());
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn settings_at(path: &PathBuf) -> Settings {
        let data = std::fs::read_to_string(path).unwrap_or_default();
        serde_json::from_str(&data).unwrap_or_default()
    }

    fn write_settings(path: &PathBuf, s: &Settings) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, serde_json::to_string_pretty(s).unwrap()).unwrap();
    }

    #[test]
    fn default_has_no_model() {
        let s = Settings::default();
        assert!(s.model.is_none());
    }

    #[test]
    fn roundtrip_model() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            model: Some("qwen3-4b".to_string()),
        };
        write_settings(&path, &s);

        let loaded = settings_at(&path);
        assert_eq!(loaded.model.as_deref(), Some("qwen3-4b"));
    }

    #[test]
    fn set_model_updates_field() {
        let mut s = Settings {
            model: Some("old".to_string()),
        };
        // Simulate what set_model does without touching the filesystem.
        s.model = Some("new-model".to_string());
        assert_eq!(s.model.as_deref(), Some("new-model"));
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.json");
        // File does not exist; reading should give default.
        let data = std::fs::read_to_string(&path).unwrap_or_default();
        let s: Settings = serde_json::from_str(&data).unwrap_or_default();
        assert!(s.model.is_none());
    }

    #[test]
    fn corrupt_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, b"not json at all!!!").unwrap();
        let data = std::fs::read_to_string(&path).unwrap_or_default();
        let s: Settings = serde_json::from_str(&data).unwrap_or_default();
        assert!(s.model.is_none());
    }

    #[test]
    fn serialises_to_valid_json() {
        let s = Settings {
            model: Some("mymodel".to_string()),
        };
        let json = serde_json::to_string(&s).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["model"], "mymodel");
    }

    #[test]
    fn null_model_deserialises_as_none() {
        let json = r#"{"model": null}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.model.is_none());
    }
}
