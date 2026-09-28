//! Settings loading: `~/.config/json-editor/config.json`, or the file given
//! with `--config`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ratatui_json_editor::Json;
use ratatui_textarea::Key;

use crate::keymap::{default_binding, Binding};
use crate::menu::Action;

/// Settings from `~/.config/json-editor/config.json`.
#[derive(Debug, Default)]
pub(crate) struct Config {
    /// Save the file automatically on exit.
    pub(crate) autosave: bool,
    /// The key each action runs on, defaults where the config says nothing.
    pub(crate) binds: Vec<(Action, Binding)>,
}

impl Config {
    /// The key an action runs on: the configured one, or the default.
    pub(crate) fn binding(&self, action: Action) -> Binding {
        self.binds
            .iter()
            .find(|(bound, _)| *bound == action)
            .map(|(_, binding)| *binding)
            .unwrap_or_else(|| default_binding(action))
    }
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
            ("binds", Json::Object(binds)) => config.binds = parse_binds(binds)?,
            ("binds", _) => return Err("`binds` must be an object".to_string()),
            (name, _) => return Err(format!("unknown option {name:?}")),
        }
    }
    Ok(config)
}

/// Reads the `binds` object: one entry per action, each naming a key and
/// optionally the modifiers to hold with it. All three default to false.
fn parse_binds(entries: &[(String, Json)]) -> Result<Vec<(Action, Binding)>, String> {
    let mut binds = Vec::new();
    for (name, value) in entries {
        let Json::Object(fields) = value else {
            return Err(format!("the binding for {name:?} must be an object"));
        };
        let mut key = None;
        // Modifiers are taken as written and are false when not named, so a
        // rebind never inherits anything from the action's default.
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        for (field, setting) in fields {
            match field.as_str() {
                "key" => {
                    let Json::String(name) = setting else {
                        return Err(format!("the key for {name:?} must be a string"));
                    };
                    key = Some(parse_key(name)?);
                }
                "ctrl" => ctrl = parse_flag(name, field, setting)?,
                "alt" => alt = parse_flag(name, field, setting)?,
                "shift" => shift = parse_flag(name, field, setting)?,
                other => {
                    return Err(format!("unknown binding field {other:?} for {name:?}"));
                }
            }
        }
        let Some(key) = key else {
            return Err(format!("the binding for {name:?} needs a \"key\""));
        };
        let Some(binding) = Binding::from_json(name, key, ctrl, alt, shift) else {
            return Err(format!("unknown action {name:?}"));
        };
        binds.push((Action::from_name(name).unwrap(), binding));
    }
    Ok(binds)
}

fn parse_flag(action: &str, field: &str, setting: &Json) -> Result<bool, String> {
    match setting {
        Json::Bool(flag) => Ok(*flag),
        _ => Err(format!("{field:?} for {action:?} must be true or false")),
    }
}

/// The names a key may be written as: a single character, a name like `f2` or
/// `tab`, or one of the arrow and editing keys.
fn parse_key(name: &str) -> Result<Key, String> {
    let mut chars = name.chars();
    let Some(one) = chars.next() else {
        return Err("a key cannot be empty".to_string());
    };
    if chars.next().is_none() {
        return Ok(Key::Char(one));
    }
    Ok(match name {
        "esc" | "escape" => Key::Esc,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "insert" => Key::Home,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" | "page_up" => Key::PageUp,
        "pagedown" | "page_down" => Key::PageDown,
        "up" => Key::Up,
        "down" => Key::Down,
        "left" => Key::Left,
        "right" => Key::Right,
        "space" => Key::Char(' '),
        _ => {
            // `f1` through `f12`, and nothing else.
            let number = name
                .strip_prefix('f')
                .filter(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
                .and_then(|rest| rest.parse::<u8>().ok())
                .filter(|number| (1..=12).contains(number));
            match number {
                Some(number) => Key::F(number),
                None => return Err(format!("unknown key {name:?}")),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_textarea::Key;

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
    fn binds_override_the_defaults() {
        let config = parse_config(
            r#"{"binds": {"add": {"key": "i", "ctrl": true}, "save": {"key": "w"}}}"#,
        )
        .unwrap();
        let add = config.binding(Action::Add);
        assert_eq!(add.key, Key::Char('i'));
        assert!(add.ctrl);
        let save = config.binding(Action::Save);
        assert_eq!(
            save,
            Binding::plain(Key::Char('w')),
            "save's Ctrl+S default is not inherited by the new key"
        );
    }

    #[test]
    fn bind_modifiers_default_to_false() {
        let config = parse_config(r#"{"binds": {"add": {"key": "i"}}}"#).unwrap();
        let add = config.binding(Action::Add);
        assert_eq!(add, Binding::plain(Key::Char('i')));
    }

    #[test]
    fn a_named_modifier_is_taken_as_written() {
        // Naming a modifier sets it, and leaving it out means no modifier —
        // the same either way, so Ctrl is not silently kept.
        let config = parse_config(r#"{"binds": {"save": {"key": "s", "ctrl": false}}}"#).unwrap();
        assert_eq!(config.binding(Action::Save), Binding::plain(Key::Char('s')));

        let config = parse_config(r#"{"binds": {"save": {"key": "s"}}}"#).unwrap();
        assert_eq!(config.binding(Action::Save), Binding::plain(Key::Char('s')));

        let config = parse_config(r#"{"binds": {"save": {"key": "s", "ctrl": true}}}"#).unwrap();
        assert!(config.binding(Action::Save).ctrl);
    }

    #[test]
    fn a_shifted_key_in_a_config_still_needs_shift() {
        // `J` is sent with Shift held, so the config has to say so.
        let config = parse_config(r#"{"binds": {"move-down": {"key": "n"}}}"#).unwrap();
        assert!(!config.binding(Action::MoveDown).shift, "a plain n needs no shift");

        let config = parse_config(r#"{"binds": {"move-down": {"key": "N"}}}"#).unwrap();
        let binding = config.binding(Action::MoveDown);
        assert_eq!(binding.key, Key::Char('N'));
        assert!(!binding.shift, "the character is already the shifted one");
    }

    #[test]
    fn an_action_with_no_binding_keeps_its_default() {
        let config = parse_config(r#"{"binds": {"add": {"key": "i"}}}"#).unwrap();
        assert_eq!(config.binding(Action::Delete), default_binding(Action::Delete));
    }

    #[test]
    fn binds_are_validated() {
        for bad in [
            r#"{"binds": {"nope": {"key": "x"}}}"#,
            r#"{"binds": {"add": {}}}"#,
            r#"{"binds": {"add": {"key": 1}}}"#,
            r#"{"binds": {"add": {"key": "x", "ctrl": 1}}}"#,
            r#"{"binds": {"add": {"key": "x", "hyper": true}}}"#,
            r#"{"binds": {"add": "x"}}"#,
            r#"{"binds": []}"#,
            r#"{"binds": {"add": {"key": "nonsense"}}}"#,
        ] {
            assert!(parse_config(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn key_names_are_understood() {
        for (text, key) in [
            ("x", Key::Char('x')),
            ("space", Key::Char(' ')),
            ("f10", Key::F(10)),
            ("tab", Key::Tab),
            ("esc", Key::Esc),
            ("enter", Key::Enter),
            ("up", Key::Up),
            ("page_up", Key::PageUp),
            ("pagedown", Key::PageDown),
        ] {
            let config = parse_config(&format!(r#"{{"binds": {{"add": {{"key": "{text}"}}}}}}"#))
                .unwrap_or_else(|err| panic!("{text}: {err}"));
            assert_eq!(config.binding(Action::Add).key, key, "{text}");
        }
    }

    #[test]
    fn a_rebound_save_does_not_keep_ctrl() {
        // The rule the docs promise, checked through the loader.
        let dir = std::env::temp_dir().join("json-editor-rebind-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"binds": {"save": {"key": "w"}}}"#).unwrap();
        let config = load_config(Some(&path)).unwrap();
        assert_eq!(
            config.binding(Action::Save),
            Binding::plain(Key::Char('w')),
            "w saves, with no Ctrl"
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_config_file_rebinds_keys() {
        // The shape the docs show, loaded the way the app loads it.
        let dir = std::env::temp_dir().join("json-editor-binds-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{
                 "autosave": true,
                 "binds": { "delete": { "key": "x", "ctrl": true } }
               }"#,
        )
        .unwrap();

        let config = load_config(Some(&path)).unwrap();
        assert!(config.autosave);
        let delete = config.binding(Action::Delete);
        assert_eq!(delete.key, Key::Char('x'));
        assert!(delete.ctrl, "named in the config");
        assert!(!delete.alt, "not named, so false");
        assert!(!delete.shift, "not named, so false");
        // Everything else is untouched.
        assert_eq!(config.binding(Action::Add), default_binding(Action::Add));
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn explicit_config_paths_must_exist() {
        let err = load_config(Some(Path::new("/nonexistent/config.json"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("/nonexistent/config.json"));
    }
}
