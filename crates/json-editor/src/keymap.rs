//! Keybindings: which key runs which action, and the defaults.
//!
//! Every action has a default binding. A config can replace any of them, and
//! the table is the single place that says what an action is called and what
//! it is bound to.

use ratatui_textarea::{Input, Key};

use crate::menu::Action;

/// A key and its modifiers. Modifiers are false unless the config says
/// otherwise, so `"key": "s"` is plain `s` and `"key": "s", "ctrl": true` is
/// `Ctrl+S`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Binding {
    pub(crate) key: Key,
    pub(crate) ctrl: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
}

impl Binding {
    pub(crate) fn plain(key: Key) -> Self {
        Self {
            key,
            ..Self::default()
        }
    }

    fn ctrl(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
            ..Self::default()
        }
    }

    /// A key a terminal sends with Shift held, because the character itself is
    /// the shifted one: `K` for `Shift+k`, `+` for `Shift+=`.
    fn shifted(key: Key) -> Self {
        Self {
            key,
            shift: true,
            ..Self::default()
        }
    }

    /// Whether an input event is this binding. The `shift` flag is a property
    /// of the key rather than a modifier, so a plain `a` and a `Shift+a` are
    /// different bindings and neither is caught by the other.
    pub(crate) fn matches(&self, input: &Input) -> bool {
        self.key == input.key && self.ctrl == input.ctrl && self.alt == input.alt
    }

    /// The binding a config entry names, if it names a real action.
    ///
    /// Every modifier is taken exactly as written and is false when the
    /// config leaves it out, so a rebind never inherits anything from the
    /// default: `{"save": {"key": "w"}}` is a plain `w`, not `Ctrl+W`.
    pub(crate) fn from_json(name: &str, key: Key, ctrl: bool, alt: bool, shift: bool) -> Option<Self> {
        Action::from_name(name)?;
        Some(Self {
            key,
            ctrl,
            alt,
            shift,
        })
    }
}

/// The key each action runs on. The order is the order they appear in the
/// menus, so a list of them reads as a reference.
macro_rules! defaults {
    ($($action:ident => $key:expr;)*) => {
        /// The default binding of every action.
        pub(crate) fn default_binding(action: Action) -> Binding {
            match action {
                $(Action::$action => $key,)*
            }
        }
    };
}

defaults! {
    Save => Binding::ctrl(Key::Char('s'));
    Quit => Binding::plain(Key::Esc);
    Add => Binding::plain(Key::Char('a'));
    Delete => Binding::plain(Key::Char('d'));
    MoveUp => Binding::shifted(Key::Char('K'));
    MoveDown => Binding::shifted(Key::Char('J'));
    MoveAcrossUp => Binding::shifted(Key::Char('H'));
    MoveAcrossDown => Binding::shifted(Key::Char('L'));
    EditValue => Binding::plain(Key::Char('e'));
    EditKey => Binding::plain(Key::Char('r'));
    HideBlock => Binding::plain(Key::Char('-'));
    ShowBlock => Binding::shifted(Key::Char('+'));
    ToggleBlock => Binding::shifted(Key::Char('*'));
    SelectUp => Binding::plain(Key::Char('k'));
    SelectDown => Binding::plain(Key::Char('j'));
    SelectLeft => Binding::plain(Key::Char('h'));
    SelectRight => Binding::plain(Key::Char('l'));
    SelectLineUp => Binding::plain(Key::Up);
    SelectLineDown => Binding::plain(Key::Down);
    SelectWordLeft => Binding::plain(Key::Left);
    SelectWordRight => Binding::plain(Key::Right);
    SelectFirst => Binding::plain(Key::Home);
    SelectLast => Binding::plain(Key::End);
    PageUp => Binding::plain(Key::PageUp);
    PageDown => Binding::plain(Key::PageDown);
    EditField => Binding::plain(Key::F(2));
    Menu => Binding::plain(Key::F(10));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_default() {
        for action in Action::all() {
            // A binding that matches nothing is a mistake in the table.
            assert_ne!(default_binding(action), Binding::plain(Key::Null));
        }
    }

    #[test]
    fn modifiers_default_to_false() {
        let binding = Binding::from_json("add", Key::Char('a'), false, false, false).unwrap();
        assert_eq!(binding, Binding::plain(Key::Char('a')));
        assert!(!binding.ctrl && !binding.alt && !binding.shift);
    }

    #[test]
    fn a_binding_matches_its_key_and_modifiers() {
        let binding = Binding::from_json("save", Key::Char('s'), true, false, false).unwrap();
        assert!(binding.matches(&Input {
            key: Key::Char('s'),
            ctrl: true,
            alt: false,
            shift: false,
        }));
        assert!(!binding.matches(&Input {
            key: Key::Char('s'),
            ctrl: false,
            alt: false,
            shift: false,
        }), "Ctrl+S is not s");
    }

    #[test]
    fn a_rebind_does_not_inherit_a_default_modifier() {
        // Save is Ctrl+S by default, but naming another key with no modifier
        // gives a plain one.
        let binding = Binding::from_json("save", Key::Char('w'), false, false, false).unwrap();
        assert_eq!(binding, Binding::plain(Key::Char('w')));
    }

    #[test]
    fn shifted_keys_declare_their_shift() {
        // A terminal sends `J` with Shift held, so a binding for it has to say
        // so or the key never matches.
        for action in [Action::MoveUp, Action::MoveDown, Action::ShowBlock] {
            let binding = default_binding(action);
            assert!(binding.shift, "{} is a shifted key", action.name());
            assert!(binding.matches(&Input {
                key: binding.key,
                ctrl: false,
                alt: false,
                shift: true,
            }));
        }
    }

    #[test]
    fn unknown_action_names_are_rejected() {
        assert!(Binding::from_json("nope", Key::Char('x'), false, false, false).is_none());
    }
}
