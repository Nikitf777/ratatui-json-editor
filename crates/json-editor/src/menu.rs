//! The top menu: the button tree and the bar/dropdown geometry that `tui-menu`
//! draws, which the mouse hit-testing mirrors.

use tui_menu::{MenuItem, MenuState};

/// What the menu buttons run. The JSON ones mirror the `a`/`d`/`J`/`K`/`e`/`r`
/// keys: every action that changes the document lives here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Save,
    Quit,
    Add,
    Delete,
    MoveUp,
    MoveDown,
    MoveAcrossUp,
    MoveAcrossDown,
    EditValue,
    EditKey,
    HideBlock,
    ShowBlock,
    ToggleBlock,
    SelectUp,
    SelectDown,
    SelectLeft,
    SelectRight,
    SelectLineUp,
    SelectLineDown,
    SelectWordLeft,
    SelectWordRight,
    SelectFirst,
    SelectLast,
    PageUp,
    PageDown,
    EditField,
    Menu,
}

impl Action {
    /// The name used in the config's `binds`, and in error messages.
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Action::Save => "save",
            Action::Quit => "quit",
            Action::Add => "add",
            Action::Delete => "delete",
            Action::MoveUp => "move-up",
            Action::MoveDown => "move-down",
            Action::MoveAcrossUp => "move-across-up",
            Action::MoveAcrossDown => "move-across-down",
            Action::EditValue => "edit-value",
            Action::EditKey => "edit-key",
            Action::HideBlock => "hide-block",
            Action::ShowBlock => "show-block",
            Action::ToggleBlock => "toggle-block",
            Action::SelectUp => "select-up",
            Action::SelectDown => "select-down",
            Action::SelectLeft => "select-left",
            Action::SelectRight => "select-right",
            Action::SelectLineUp => "select-line-up",
            Action::SelectLineDown => "select-line-down",
            Action::SelectWordLeft => "select-word-left",
            Action::SelectWordRight => "select-word-right",
            Action::SelectFirst => "select-first",
            Action::SelectLast => "select-last",
            Action::PageUp => "page-up",
            Action::PageDown => "page-down",
            Action::EditField => "edit-field",
            Action::Menu => "menu",
        }
    }

    /// The action a config name refers to, or `None` when it names none.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::all().into_iter().find(|action| action.name() == name)
    }

    /// Every action, in the order they appear in the menus and then the
    /// navigation keys, so a listing of them reads as a reference.
    pub(crate) fn all() -> Vec<Self> {
        vec![
            Action::Save,
            Action::Quit,
            Action::Add,
            Action::Delete,
            Action::MoveUp,
            Action::MoveDown,
            Action::MoveAcrossUp,
            Action::MoveAcrossDown,
            Action::EditValue,
            Action::EditKey,
            Action::HideBlock,
            Action::ShowBlock,
            Action::ToggleBlock,
            Action::SelectUp,
            Action::SelectDown,
            Action::SelectLeft,
            Action::SelectRight,
            Action::SelectLineUp,
            Action::SelectLineDown,
            Action::SelectWordLeft,
            Action::SelectWordRight,
            Action::SelectFirst,
            Action::SelectLast,
            Action::PageUp,
            Action::PageDown,
            Action::EditField,
            Action::Menu,
        ]
    }
}

const FILE_MENU: &[(Action, &str)] = &[(Action::Save, "Save"), (Action::Quit, "Quit")];
const EDIT_MENU: &[(Action, &str)] = &[
    (Action::Add, "Add entry"),
    (Action::Delete, "Delete entry"),
    (Action::MoveUp, "Move entry up"),
    (Action::MoveDown, "Move entry down"),
    (Action::MoveAcrossUp, "Move across up"),
    (Action::MoveAcrossDown, "Move across down"),
    (Action::EditValue, "Edit value"),
    (Action::EditKey, "Edit key"),
];
const VIEW_MENU: &[(Action, &str)] = &[
    (Action::HideBlock, "Hide block"),
    (Action::ShowBlock, "Show block"),
    (Action::ToggleBlock, "Toggle block"),
];
pub(crate) const MENUS: &[(&str, &[(Action, &str)])] = &[
    ("File", FILE_MENU),
    ("Edit", EDIT_MENU),
    ("View", VIEW_MENU),
];

pub(crate) fn build_menu() -> MenuState<Action> {
    MenuState::new(
        MENUS
            .iter()
            .map(|(name, items)| {
                MenuItem::group(
                    *name,
                    items
                        .iter()
                        .map(|(action, label)| MenuItem::item(*label, *action))
                        .collect(),
                )
            })
            .collect(),
    )
}

/// Where a menu title starts on the bar: `tui-menu` draws one blank and then
/// ` name ` per menu.
pub(crate) fn title_x(index: usize) -> u16 {
    1 + MENUS[..index]
        .iter()
        .map(|(name, _)| name.len() as u16 + 2)
        .sum::<u16>()
}
