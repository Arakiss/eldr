//! `~/.config/eldr/config.toml` read as a flat KEY=value file (hand-parsed; no
//! `toml`/`serde` crates). Only the watchdog arming flags and thresholds live here.
//! Fully wired in M4/M5; M0 provides the loader skeleton.

use std::collections::HashMap;

/// Flat config: `key = value`, `#` comments, blank lines ignored. We accept both
/// `KEY=value` and `key = "value"` forms so the bash `watchdog.conf` ports cleanly.
#[derive(Clone, Default, Debug)]
pub struct Config {
    map: HashMap<String, String>,
}

impl Config {
    /// Load from the default path, or an empty config if absent.
    pub fn load() -> Self {
        let path = default_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => Config::parse(&text),
            Err(_) => Config::default(),
        }
    }

    pub fn parse(text: &str) -> Self {
        let mut map = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Strip trailing inline comments that are clearly comments (after value).
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let key = k.trim().to_string();
            let mut val = v.trim();
            // strip surrounding quotes
            if (val.starts_with('"') && val.ends_with('"') && val.len() >= 2)
                || (val.starts_with('\'') && val.ends_with('\'') && val.len() >= 2)
            {
                val = &val[1..val.len() - 1];
            }
            map.insert(key, val.to_string());
        }
        Config { map }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|s| s.as_str())
    }

    /// Read a flag that defaults to `default` when unset. `1`/`true`/`yes` are true.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.get(key) {
            Some(v) => matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            None => default,
        }
    }

    pub fn int(&self, key: &str, default: i64) -> i64 {
        self.get(key)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }

    pub fn float(&self, key: &str, default: f64) -> f64 {
        self.get(key)
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(default)
    }
}

/// `$XDG_CONFIG_HOME/eldr/config.toml` or `~/.config/eldr/config.toml`.
pub fn default_path() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("ELDR_CONF") {
        return std::path::PathBuf::from(x);
    }
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("{}/.config", home()));
    std::path::PathBuf::from(base)
        .join("eldr")
        .join("config.toml")
}

/// `$HOME`, or `/tmp` as a last resort.
pub fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

/// Data directory: `$ELDR_DIR` or `~/.local/share/eldr`. Holds status.json,
/// alerts.log, actions.log, processes.log.
pub fn data_dir() -> std::path::PathBuf {
    if let Ok(x) = std::env::var("ELDR_DIR") {
        return std::path::PathBuf::from(x);
    }
    std::path::PathBuf::from(home())
        .join(".local")
        .join("share")
        .join("eldr")
}

/// Ensure the data directory exists; returns its path.
pub fn ensure_data_dir() -> std::path::PathBuf {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn status_path() -> std::path::PathBuf {
    data_dir().join("status.json")
}
/// Rolling telemetry series (cpu_load, fan_rpm, sys_power) the guard appends to, so the
/// TUI can open with its sparklines already populated.
pub fn history_path() -> std::path::PathBuf {
    data_dir().join("history.csv")
}
pub fn alerts_path() -> std::path::PathBuf {
    data_dir().join("alerts.log")
}
pub fn actions_path() -> std::path::PathBuf {
    data_dir().join("actions.log")
}
pub fn processes_path() -> std::path::PathBuf {
    data_dir().join("processes.log")
}
pub fn pid_path() -> std::path::PathBuf {
    data_dir().join("guard.pid")
}
