//! The top menu: the button tree and the bar/dropdown geometry that `tui-menu`
//! draws, which the mouse hit-testing mirrors.

use tui_menu::{MenuItem, MenuState};

/// What the menu buttons run. The JSON ones mirror the `a`/`d`/`J`/`K`/`e`/`r`
/// keys: every action that changes the document lives here.
#[derive(Clone)]
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
pub(crate) const MENUS: &[(&str, &[(Action, &str)])] = &[("File", FILE_MENU), ("Edit", EDIT_MENU)];

pub(crate) fn build_menu() -> MenuState<Action> {
    MenuState::new(
        MENUS
            .iter()
            .map(|(name, items)| {
                MenuItem::group(
                    *name,
                    items
                        .iter()
                        .map(|(action, label)| MenuItem::item(*label, action.clone()))
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
