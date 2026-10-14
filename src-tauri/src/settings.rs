//! User settings for the activity view, saved beside the cache DB.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// The local model the "local candidate" flag is judged against. URL and model
    /// are also what the optional labelling pass uses.
    pub ollama_url: String,
    pub ollama_model: String,
    /// Largest context (input + cache) the local model can take, in tokens.
    pub local_context_limit: i64,
    /// Largest single answer the local model is trusted to write, in tokens.
    pub local_output_limit: i64,
    /// The optional labelling pass by the local model (labeller.rs). Off by default.
    pub model_labels_enabled: bool,
    /// Let the pass use a non-loopback Ollama URL. Segment excerpts then leave
    /// this machine; the card warns. Off by default.
    pub allow_remote_ollama: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        AppSettings {
            ollama_url: "http://127.0.0.1:11434".into(),
            ollama_model: String::new(),
            local_context_limit: 32_000,
            local_output_limit: 4_000,
            model_labels_enabled: false,
            allow_remote_ollama: false,
        }
    }
}

impl AppSettings {
    /// Saved settings, or defaults.
    pub fn load_or_default(path: &Path) -> AppSettings {
        std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_string_pretty(self).map_err(std::io::Error::other)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_until_saved_then_saved_settings_win() {
        let dir = std::env::temp_dir().join(format!("al_settings_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");

        let s = AppSettings::load_or_default(&path);
        assert_eq!(s, AppSettings::default());
        assert_eq!(s.local_context_limit, 32_000);

        let mine = AppSettings { local_context_limit: 128_000, ollama_model: "qwen3:32b".into(), ..s };
        mine.save(&path).unwrap();
        assert_eq!(AppSettings::load_or_default(&path), mine);

        // A file saved by an older build with a field this one no longer has still loads.
        std::fs::write(&path, r#"{"ollama_model":"qwen3:32b","retired_field":"x"}"#).unwrap();
        assert_eq!(AppSettings::load_or_default(&path).ollama_model, "qwen3:32b");
        std::fs::remove_dir_all(&dir).ok();
    }
}
