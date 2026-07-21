//! Load/save the config to JSON on disk and in memory.

use std::path::Path;

use crate::error::ConfigError;
use crate::model::Config;

/// Parse a config from a JSON string.
pub fn from_json_str(s: &str) -> Result<Config, ConfigError> {
    Ok(serde_json::from_str(s)?)
}

/// Serialize a config to pretty JSON.
pub fn to_json_string(cfg: &Config) -> Result<String, ConfigError> {
    Ok(serde_json::to_string_pretty(cfg)?)
}

/// Read and parse a config file.
pub fn load_file(path: &Path) -> Result<Config, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    from_json_str(&text)
}

/// Serialize and write a config file.
pub fn save_file(path: &Path, cfg: &Config) -> Result<(), ConfigError> {
    let text = to_json_string(cfg)?;
    std::fs::write(path, text).map_err(|source| ConfigError::Io {
        path: path.display().to_string(),
        source,
    })
}
