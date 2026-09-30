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

    /// A shifted key with Ctrl held as well: `Ctrl+Shift+C` arrives as `C`
    /// with both modifiers set.
    fn ctrl_shifted(key: Key) -> Self {
        Self {
            key,
            ctrl: true,
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

    /// The binding a config entry names.
    ///
    /// Every modifier is taken exactly as written and is false when the
    /// config leaves it out, so a rebind never inherits anything from the
    /// default: `{"save": {"key": "w"}}` is a plain `w`, not `Ctrl+W`.
    pub(crate) fn from_json(key: Key, ctrl: bool, alt: bool, shift: bool) -> Self {
        Self {
            key,
            ctrl,
            alt,
            shift,
        }
    }
}

/// The names a key may be written as: a single character, a name like `f2` or
/// `tab`, or one of the arrow and editing keys. `None` for anything else.
pub(crate) fn key_from_name(name: &str) -> Option<Key> {
    let mut chars = name.chars();
    let one = chars.next()?;
    if chars.next().is_none() {
        return Some(Key::Char(one));
    }
    Some(match name {
        "esc" | "escape" => Key::Esc,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
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
            Key::F(number?)
        }
    })
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
    Duplicate => Binding::ctrl(Key::Char('d'));
    Unflatten => Binding::shifted(Key::Char('D'));
    CopyEntry => Binding::ctrl(Key::Char('c'));
    // Copying a key or a value on its own is not bound by default: the two
    // other copy buttons cover the usual cases, and `Ctrl+K`/`Ctrl+V` are
    // free for a config to use instead.
    CopyKey => Binding::plain(Key::Null);
    CopyValue => Binding::plain(Key::Null);
    CopySelected => Binding::ctrl_shifted(Key::Char('C'));
    ValueToString => Binding::ctrl(Key::Char('t'));
    StringToValue => Binding::ctrl(Key::Char('y'));
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
    fn no_two_actions_share_a_default() {
        // A key runs the first action bound to it, so a duplicate here would
        // quietly make one of them unreachable.
        let mut seen: Vec<(Action, Binding)> = Vec::new();
        for action in Action::all() {
            let binding = default_binding(action);
            if binding == Binding::plain(Key::Null) {
                continue;
            }
            if let Some((other, _)) = seen.iter().find(|(_, taken)| *taken == binding) {
                panic!("{other:?} and {action:?} are both bound to {binding:?}");
            }
            seen.push((action, binding));
        }
    }

    #[test]
    fn an_unbound_action_has_no_key() {
        // `Key::Null` is what an unbound action is given: no terminal sends
        // it, so the action is only ever run from the menu or from a key the
        // config gives it.
        assert_eq!(default_binding(Action::CopyKey), Binding::plain(Key::Null));
        assert_eq!(
            default_binding(Action::CopyValue),
            Binding::plain(Key::Null)
        );
    }

    #[test]
    fn modifiers_default_to_false() {
        let binding = Binding::from_json(Key::Char('a'), false, false, false);
        assert_eq!(binding, Binding::plain(Key::Char('a')));
        assert!(!binding.ctrl && !binding.alt && !binding.shift);
    }

    #[test]
    fn a_binding_matches_its_key_and_modifiers() {
        let binding = Binding::from_json(Key::Char('s'), true, false, false);
        assert!(binding.matches(&Input {
            key: Key::Char('s'),
            ctrl: true,
            alt: false,
            shift: false,
        }));
        assert!(
            !binding.matches(&Input {
                key: Key::Char('s'),
                ctrl: false,
                alt: false,
                shift: false,
            }),
            "Ctrl+S is not s"
        );
    }

    #[test]
    fn a_rebind_does_not_inherit_a_default_modifier() {
        // Save is Ctrl+S by default, but naming another key with no modifier
        // gives a plain one.
        let binding = Binding::from_json(Key::Char('w'), false, false, false);
        assert_eq!(binding, Binding::plain(Key::Char('w')));
    }

    #[test]
    fn shifted_keys_declare_their_shift() {
        // A terminal sends `J` with Shift held, so a binding for it has to say
        // so or the key never matches.
        for action in [Action::MoveUp, Action::MoveDown, Action::ShowBlock] {
            let binding = default_binding(action);
            assert!(binding.shift, "{action:?} is a shifted key");
            assert!(binding.matches(&Input {
                key: binding.key,
                ctrl: false,
                alt: false,
                shift: true,
            }));
        }
    }

    #[test]
    fn unknown_key_names_are_rejected() {
        assert!(key_from_name("nonsense").is_none());
        assert!(key_from_name("f13").is_none());
        assert!(key_from_name("f0").is_none());
        assert_eq!(key_from_name("f1"), Some(Key::F(1)));
        assert_eq!(key_from_name("page_down"), Some(Key::PageDown));
    }
}
