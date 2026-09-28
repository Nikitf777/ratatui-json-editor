//! Settings loading: `~/.config/json-editor/config.json`, or the file given
//! with `--config`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::keymap::{Binding, default_binding, key_from_name};
use crate::menu::Action;

/// Settings from `~/.config/json-editor/config.json`.
///
/// Unknown options are rejected rather than ignored, so a typo is caught
/// instead of quietly doing nothing.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// Save the file automatically on exit.
    #[serde(default)]
    pub(crate) autosave: bool,
    /// The key each action runs on. An action with no entry keeps its default.
    #[serde(default)]
    pub(crate) binds: Binds,
}

/// The key each action runs on.
#[derive(Debug, Default)]
pub(crate) struct Binds(pub(crate) Vec<(Action, Binding)>);

impl<'de> Deserialize<'de> for Binds {
    /// Reads `{"<action>": {"key": ..., "ctrl": ..., ...}}`. Modifiers left
    /// out are false, so a rebind never inherits the default's.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// One action's binding as it is written in the config.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Entry {
            key: String,
            #[serde(default)]
            ctrl: bool,
            #[serde(default)]
            alt: bool,
            #[serde(default)]
            shift: bool,
        }
        // The keys are the action names, so serde matches them against the
        // enum and reports an unknown one by name.
        let raw = std::collections::BTreeMap::<Action, Entry>::deserialize(deserializer)?;
        let mut binds = Vec::new();
        for (action, entry) in raw {
            let Some(key) = key_from_name(&entry.key) else {
                return Err(serde::de::Error::custom(format!(
                    "unknown key {:?} for {:?}",
                    entry.key, action
                )));
            };
            binds.push((
                action,
                Binding::from_json(key, entry.ctrl, entry.alt, entry.shift),
            ));
        }
        Ok(Binds(binds))
    }
}

impl Binds {
    /// The configured binding of an action, if it has one.
    fn get(&self, action: Action) -> Option<Binding> {
        self.0
            .iter()
            .find(|(bound, _)| *bound == action)
            .map(|(_, binding)| *binding)
    }
}

impl Config {
    /// The key an action runs on: the configured one, or the default.
    pub(crate) fn binding(&self, action: Action) -> Binding {
        self.binds
            .get(action)
            .unwrap_or_else(|| default_binding(action))
    }
}

/// Reads the config text. It has to be a JSON object: a list or a bare value
/// is a mistake worth reporting rather than a config with no options.
fn parse(text: &str) -> Result<Config, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    if !value.is_object() {
        return Err("the config must be a JSON object".to_string());
    }
    serde_json::from_value(value).map_err(|err| err.to_string())
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
    parse(&text).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {err}", path.display()),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui_textarea::Key;

    #[test]
    fn config_defaults_to_autosave_off() {
        assert!(!parse("{}").unwrap().autosave);
        assert!(parse(r#"{"autosave": true}"#).unwrap().autosave);
        assert!(!parse(r#"{"autosave": false}"#).unwrap().autosave);
    }

    #[test]
    fn config_rejects_mistakes() {
        assert!(parse(r#"{"autosave": 1}"#).is_err());
        assert!(parse(r#"{"autosavee": true}"#).is_err(), "typos are caught");
        assert!(parse("[]").is_err(), "the config must be an object");
    }

    #[test]
    fn binds_override_the_defaults() {
        let config =
            parse(r#"{"binds": {"add": {"key": "i", "ctrl": true}, "save": {"key": "w"}}}"#)
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
        let config = parse(r#"{"binds": {"add": {"key": "i"}}}"#).unwrap();
        assert_eq!(config.binding(Action::Add), Binding::plain(Key::Char('i')));
    }

    #[test]
    fn a_named_modifier_is_taken_as_written() {
        // Naming a modifier sets it, and leaving it out means no modifier —
        // the same either way, so Ctrl is not silently kept.
        let config = parse(r#"{"binds": {"save": {"key": "s", "ctrl": false}}}"#).unwrap();
        assert_eq!(config.binding(Action::Save), Binding::plain(Key::Char('s')));

        let config = parse(r#"{"binds": {"save": {"key": "s"}}}"#).unwrap();
        assert_eq!(config.binding(Action::Save), Binding::plain(Key::Char('s')));

        let config = parse(r#"{"binds": {"save": {"key": "s", "ctrl": true}}}"#).unwrap();
        assert!(config.binding(Action::Save).ctrl);
    }

    #[test]
    fn a_shifted_key_in_a_config_still_needs_shift() {
        let config = parse(r#"{"binds": {"move_down": {"key": "n"}}}"#).unwrap();
        assert!(
            !config.binding(Action::MoveDown).shift,
            "a plain n needs no shift"
        );

        let config = parse(r#"{"binds": {"move_down": {"key": "N"}}}"#).unwrap();
        let binding = config.binding(Action::MoveDown);
        assert_eq!(binding.key, Key::Char('N'));
        assert!(!binding.shift, "the character is already the shifted one");
    }

    #[test]
    fn an_action_with_no_binding_keeps_its_default() {
        let config = parse(r#"{"binds": {"add": {"key": "i"}}}"#).unwrap();
        assert_eq!(
            config.binding(Action::Delete),
            default_binding(Action::Delete)
        );
    }

    #[test]
    fn action_names_are_snake_case() {
        // The names in the docs, checked against what serde accepts.
        for name in [
            "save", "quit", "add", "delete", "move_up", "move_down", "move_across_up",
            "move_across_down", "edit_value", "edit_key", "hide_block", "show_block",
            "toggle_block", "select_up", "select_down", "select_left", "select_right",
            "select_line_up", "select_line_down", "select_word_left", "select_word_right",
            "select_first", "select_last", "page_up", "page_down", "edit_field", "menu",
        ] {
            let config = parse(&format!(r#"{{"binds": {{"{name}": {{"key": "x"}}}}}}"#))
                .unwrap_or_else(|err| panic!("{name} should be an action: {err}"));
            assert_eq!(config.binding(Action::Add), config.binding(Action::Add));
            assert_ne!(config.binding(Action::Add), Binding::plain(Key::Char('q')));
        }
    }

    #[test]
    fn an_unknown_action_is_named_in_the_error() {
        let err = parse(r#"{"binds": {"nope": {"key": "x"}}}"#).unwrap_err();
        assert!(err.contains("nope"), "{err}");
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
            assert!(parse(bad).is_err(), "{bad} should be rejected");
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
            let config = parse(&format!(r#"{{"binds": {{"add": {{"key": "{text}"}}}}}}"#))
                .unwrap_or_else(|err| panic!("{text}: {err}"));
            assert_eq!(config.binding(Action::Add).key, key, "{text}");
        }
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
        assert_eq!(config.binding(Action::Add), default_binding(Action::Add));
        std::fs::remove_file(&path).unwrap();
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
    fn explicit_config_paths_must_exist() {
        let err = load_config(Some(Path::new("/nonexistent/config.json"))).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(err.to_string().contains("/nonexistent/config.json"));
    }
}
