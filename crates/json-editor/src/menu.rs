//! The top menu: the button tree and the bar/dropdown geometry that `tui-menu`
//! draws, which the mouse hit-testing mirrors.

use serde::Deserialize;
use tui_menu::{MenuItem, MenuState};

/// What the menu buttons run. The JSON ones mirror the `a`/`d`/`J`/`K`/`e`/`r`
/// keys: every action that changes the document lives here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Action {
    Save,
    Quit,
    Add,
    Duplicate,
    Delete,
    Unflatten,
    CopyEntry,
    CopyKey,
    CopyValue,
    CopySelected,
    ValueToString,
    StringToValue,
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
    /// Every action, in the order they appear in the menus and then the
    /// navigation keys, so a listing of them reads as a reference.
    pub(crate) fn all() -> Vec<Self> {
        vec![
            Action::Save,
            Action::Quit,
            Action::Add,
            Action::Duplicate,
            Action::Delete,
            Action::Unflatten,
            Action::CopyEntry,
            Action::CopyKey,
            Action::CopyValue,
            Action::CopySelected,
            Action::ValueToString,
            Action::StringToValue,
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
    (Action::Duplicate, "Duplicate entry"),
    (Action::Delete, "Delete entry"),
    (Action::Unflatten, "Unflatten entry"),
    (Action::ValueToString, "Value to string"),
    (Action::StringToValue, "String to value"),
    (Action::MoveUp, "Move entry up"),
    (Action::MoveDown, "Move entry down"),
    (Action::MoveAcrossUp, "Move across up"),
    (Action::MoveAcrossDown, "Move across down"),
    (Action::EditValue, "Edit value"),
    (Action::EditKey, "Edit key"),
];
const CLIPBOARD_MENU: &[(Action, &str)] = &[
    (Action::CopyEntry, "Copy entry"),
    (Action::CopyKey, "Copy key"),
    (Action::CopyValue, "Copy value"),
    (Action::CopySelected, "Copy selected"),
];
const VIEW_MENU: &[(Action, &str)] = &[
    (Action::HideBlock, "Hide block"),
    (Action::ShowBlock, "Show block"),
    (Action::ToggleBlock, "Toggle block"),
];
pub(crate) const MENUS: &[(&str, &[(Action, &str)])] = &[
    ("File", FILE_MENU),
    ("Edit", EDIT_MENU),
    ("Clipboard", CLIPBOARD_MENU),
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
