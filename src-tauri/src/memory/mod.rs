// Local environment memory file
// Stores persistent information about the local machine

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Local machine memory entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub key: String,
    pub value: String,
    pub category: String,
}

/// The full memory file stored as YAML
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryStore {
    pub machine_name: String,
    pub os: String,
    pub architecture: String,
    pub hermes_endpoints: Vec<String>,
    pub entries: Vec<MemoryEntry>,
    pub last_updated: String,
}

impl MemoryStore {
    /// Load memory from the default path
    pub fn load() -> Self {
        let path = Self::memory_path();
        match fs::read_to_string(&path) {
            Ok(content) => serde_yaml::from_str(&content).unwrap_or_else(|_| Self::default_store()),
            Err(_) => Self::default_store(),
        }
    }

    /// Save memory to disk
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::memory_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_yaml::to_string(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(&path, content)
    }

    /// Add or update a memory entry
    pub fn set(&mut self, key: &str, value: &str, category: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
            entry.value = value.to_string();
            entry.category = category.to_string();
        } else {
            self.entries.push(MemoryEntry {
                key: key.to_string(),
                value: value.to_string(),
                category: category.to_string(),
            });
        }
        self.last_updated = chrono::Utc::now().to_rfc3339();
    }

    /// Get a memory entry by key
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|e| e.key == key).map(|e| e.value.as_str())
    }

    /// List entries by category
    pub fn list_by_category(&self, category: &str) -> Vec<&MemoryEntry> {
        self.entries.iter().filter(|e| e.category == category).collect()
    }

    /// Get all entries
    pub fn all_entries(&self) -> &[MemoryEntry] {
        &self.entries
    }

    fn memory_path() -> PathBuf {
        dirs_next().unwrap_or_else(|| PathBuf::from("."))
            .join(".retana")
            .join("memory.yaml")
    }

    fn default_store() -> Self {
        Self {
            machine_name: "unknown".to_string(),
            os: std::env::consts::OS.to_string(),
            architecture: std::env::consts::ARCH.to_string(),
            hermes_endpoints: Vec::new(),
            entries: Vec::new(),
            last_updated: chrono::Utc::now().to_rfc3339(),
        }
    }
}

fn dirs_next() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config"))
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(PathBuf::from)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}
