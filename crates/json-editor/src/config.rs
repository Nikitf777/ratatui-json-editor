//! Settings loading: `~/.config/json-editor/config.json`, or the file given
//! with `--config`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ratatui_json_editor::Json;

/// Settings from `~/.config/json-editor/config.json`.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// Save the file automatically on exit.
    pub(crate) autosave: bool,
}

/// Loads the config from `path`, or from `~/.config/json-editor/config.json`
/// when `path` is `None`. A missing default file means defaults; an explicit
/// path must exist so typos are caught.
pub(crate) fn load_config(path: Option<&Path>) -> io::Result<Config> {
    let (path, explicit) = match path {
        Some(path) => (path.to_path_buf(), true),
        None => {
            let Some(home) = std::env::var_os("HOME") else {
                return Ok(Config::default());
            };
            (
                PathBuf::from(home).join(".config/json-editor/config.json"),
                false,
            )
        }
    };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound && !explicit => {
            return Ok(Config::default());
        }
        Err(err) => {
            return Err(io::Error::new(
                err.kind(),
                format!("read {}: {err}", path.display()),
            ));
        }
    };
    parse_config(&text)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {err}", path.display())))
}

fn parse_config(text: &str) -> Result<Config, String> {
    let json = Json::parse(text).map_err(|err| err.to_string())?;
    let Json::Object(entries) = &json else {
        return Err("the config must be a JSON object".to_string());
    };
    let mut config = Config::default();
    for (key, value) in entries {
        match (key.as_str(), value) {
            ("autosave", Json::Bool(autosave)) => config.autosave = *autosave,
            ("autosave", _) => return Err("`autosave` must be true or false".to_string()),
            (name, _) => return Err(format!("unknown option {name:?}")),
        }
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_to_autosave_off() {
        assert!(!parse_config("{}").unwrap().autosave);
        assert!(parse_config(r#"{"autosave": true}"#).unwrap().autosave);
        assert!(!parse_config(r#"{"autosave": false}"#).unwrap().autosave);
    }

    #[test]
    fn config_rejects_mistakes() {
        assert!(parse_config(r#"{"autosave": 1}"#).is_err());
        assert!(parse_config(r#"{"autosavee": true}"#).is_err(), "typos are caught");
        assert!(parse_config("[]").is_err(), "the config must be an object");
    }

    #[test]
    fn explicit_config_paths_must_exist() {
        let err = load_config(Some(Path::new("/nonexistent/config.json"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("/nonexistent/config.json"));
    }
}
