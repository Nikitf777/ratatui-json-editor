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
    /// The keys each action runs on. An action with no entry keeps its
    /// default.
    #[serde(default)]
    pub(crate) binds: Binds,
}

/// The keys each action runs on, in the order the config listed them.
#[derive(Debug, Default)]
pub(crate) struct Binds(pub(crate) Vec<(Action, Vec<Binding>)>);

impl<'de> Deserialize<'de> for Binds {
    /// Reads `{"<action>": [ {"key": ..., "ctrl": ...}, ... ]}`. An action
    /// can be bound to several keys. Modifiers left out are false, so a
    /// rebind never inherits the default's.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// One key an action runs on, as it is written in the config.
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
        let raw = std::collections::BTreeMap::<Action, Vec<Entry>>::deserialize(deserializer)?;
        let mut binds = Vec::new();
        for (action, entries) in raw {
            if entries.is_empty() {
                return Err(serde::de::Error::custom(format!(
                    "{action:?} has no keys bound to it"
                )));
            }
            let mut parsed = Vec::new();
            for entry in entries {
                let Some(key) = key_from_name(&entry.key) else {
                    return Err(serde::de::Error::custom(format!(
                        "unknown key {:?} for {action:?}",
                        entry.key
                    )));
                };
                parsed.push(Binding::from_json(key, entry.ctrl, entry.alt, entry.shift));
            }
            binds.push((action, parsed));
        }
        Ok(Binds(binds))
    }
}

impl Binds {
    /// The configured keys of an action, if it has any.
    fn get(&self, action: Action) -> Option<&[Binding]> {
        self.0
            .iter()
            .find(|(bound, _)| *bound == action)
            .map(|(_, bindings)| bindings.as_slice())
    }
}

impl Config {
    /// The keys an action runs on: the configured ones, or its default.
    pub(crate) fn bindings(&self, action: Action) -> Vec<Binding> {
        match self.binds.get(action) {
            Some(bindings) => bindings.to_vec(),
            None => vec![default_binding(action)],
        }
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

    /// A config that is known to be good.
    fn config_of(text: &str) -> Config {
        parse(text).unwrap_or_else(|err| panic!("{text}: {err}"))
    }

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
            parse(r#"{"binds": {"add": [{"key": "i", "ctrl": true}], "save": [{"key": "w"}]}}"#)
                .unwrap();
        assert_eq!(
            config.bindings(Action::Add),
            vec![Binding::from_json(Key::Char('i'), true, false, false)]
        );
        assert_eq!(
            config.bindings(Action::Save),
            vec![Binding::plain(Key::Char('w'))],
            "save's Ctrl+S default is not inherited by the new key"
        );
    }

    #[test]
    fn an_action_can_have_several_keys() {
        let config = config_of(
            r#"{"binds": {"save": [{"key": "s", "ctrl": true}, {"key": "S", "shift": true}]}}"#,
        );
        let binds = config.bindings(Action::Save);
        assert_eq!(binds.len(), 2);
        assert_eq!(
            binds[0],
            Binding::from_json(Key::Char('s'), true, false, false)
        );
        assert_eq!(
            binds[1],
            Binding::from_json(Key::Char('S'), false, false, true)
        );
    }

    #[test]
    fn bind_modifiers_default_to_false() {
        let config = config_of(r#"{"binds": {"add": [{"key": "i"}]}}"#);
        assert_eq!(
            config.bindings(Action::Add),
            vec![Binding::plain(Key::Char('i'))]
        );
    }

    #[test]
    fn a_named_modifier_is_taken_as_written() {
        // Naming a modifier sets it, and leaving it out means no modifier —
        // the same either way, so Ctrl is not silently kept.
        let plain = vec![Binding::plain(Key::Char('s'))];
        assert_eq!(
            config_of(r#"{"binds": {"save": [{"key": "s", "ctrl": false}]}}"#)
                .bindings(Action::Save),
            plain
        );
        assert_eq!(
            config_of(r#"{"binds": {"save": [{"key": "s"}]}}"#).bindings(Action::Save),
            plain
        );
        assert_eq!(
            config_of(r#"{"binds": {"save": [{"key": "s", "ctrl": true}]}}"#)
                .bindings(Action::Save),
            vec![Binding::from_json(Key::Char('s'), true, false, false)]
        );
    }

    #[test]
    fn a_shifted_key_in_a_config_still_needs_shift() {
        let config = config_of(r#"{"binds": {"move_down": [{"key": "n"}]}}"#);
        assert!(
            !config.bindings(Action::MoveDown)[0].shift,
            "a plain n needs no shift"
        );

        let config = config_of(r#"{"binds": {"move_down": [{"key": "N"}]}}"#);
        let binding = config.bindings(Action::MoveDown)[0];
        assert_eq!(binding.key, Key::Char('N'));
        assert!(!binding.shift, "the character is already the shifted one");
    }

    #[test]
    fn an_action_with_no_binding_keeps_its_default() {
        let config = config_of(r#"{"binds": {"add": [{"key": "i"}]}}"#);
        assert_eq!(
            config.bindings(Action::Delete),
            vec![default_binding(Action::Delete)]
        );
    }

    #[test]
    fn action_names_are_snake_case() {
        // The names in the docs, checked against what serde accepts.
        for name in [
            "save",
            "quit",
            "add",
            "delete",
            "move_up",
            "move_down",
            "move_across_up",
            "move_across_down",
            "edit_value",
            "edit_key",
            "hide_block",
            "show_block",
            "toggle_block",
            "select_up",
            "select_down",
            "select_left",
            "select_right",
            "select_line_up",
            "select_line_down",
            "select_word_left",
            "select_word_right",
            "select_first",
            "select_last",
            "page_up",
            "page_down",
            "edit_field",
            "menu",
        ] {
            // Parsing is the check: an unknown name is rejected outright, so
            // every name in the docs being read here is a real action.
            config_of(&format!(r#"{{"binds": {{"{name}": [{{"key": "x"}}]}}}}"#));
        }
    }

    #[test]
    fn an_unknown_action_is_named_in_the_error() {
        let err = parse(r#"{"binds": {"nope": [{"key": "x"}]}}"#).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn binds_are_validated() {
        for bad in [
            r#"{"binds": {"nope": [{"key": "x"}]}}"#,
            r#"{"binds": {"add": []}}"#,
            r#"{"binds": {"add": {"key": "x"}}}"#,
            r#"{"binds": {"add": ["x"]}}"#,
            r#"{"binds": {"add": [{}]}}"#,
            r#"{"binds": {"add": [{"key": 1}]}}"#,
            r#"{"binds": {"add": [{"key": "x", "ctrl": 1}]}}"#,
            r#"{"binds": {"add": [{"key": "x", "hyper": true}]}}"#,
            r#"{"binds": {"add": "x"}}"#,
            r#"{"binds": []}"#,
            r#"{"binds": {"add": [{"key": "nonsense"}]}}"#,
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
            let config = config_of(&format!(r#"{{"binds": {{"add": [{{"key": "{text}"}}]}}}}"#));
            assert_eq!(config.bindings(Action::Add)[0].key, key, "{text}");
        }
    }

    #[test]
    fn several_keys_for_one_action_from_a_file() {
        let dir = std::env::temp_dir().join("json-editor-multibind-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"binds": {"save": [{"key": "s", "ctrl": true}, {"key": "S", "shift": true}]}}"#,
        )
        .unwrap();
        let config = load_config(Some(&path)).unwrap();
        assert_eq!(
            config.bindings(Action::Save),
            vec![
                Binding::from_json(Key::Char('s'), true, false, false),
                Binding::from_json(Key::Char('S'), false, false, true),
            ]
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
                 "binds": { "delete": [{ "key": "x", "ctrl": true }] }
               }"#,
        )
        .unwrap();

        let config = load_config(Some(&path)).unwrap();
        assert!(config.autosave);
        assert_eq!(
            config.bindings(Action::Delete),
            vec![Binding::from_json(Key::Char('x'), true, false, false)],
            "the named key, with the modifier it names and none it does not"
        );
        assert_eq!(
            config.bindings(Action::Add),
            vec![default_binding(Action::Add)]
        );
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_rebound_save_does_not_keep_ctrl() {
        // The rule the docs promise, checked through the loader.
        let dir = std::env::temp_dir().join("json-editor-rebind-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        std::fs::write(&path, r#"{"binds": {"save": [{"key": "w"}]}}"#).unwrap();
        let config = load_config(Some(&path)).unwrap();
        assert_eq!(
            config.bindings(Action::Save),
            vec![Binding::plain(Key::Char('w'))],
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
