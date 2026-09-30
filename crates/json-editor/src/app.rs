//! The application state machine: modes, keys, mouse, and the actions that
//! change the document.

use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui_json_editor::{EditError, Field, JsonEditorState, Theme, quote_string};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};
use tui_menu::{MenuEvent, MenuState};
use tui_scrollbar::{
    PointerButton, PointerEvent, PointerEventKind, ScrollAxis, ScrollBarInteraction, ScrollCommand,
    ScrollEvent, ScrollWheel,
};

use crate::clipboard::Clipboard;
use crate::config::Config;
use crate::format::{compact, pretty};
use crate::menu::{Action, MENUS, build_menu, title_x};
use crate::ui::{TAB_LEN, char_at, h_scrollbar, scrollbar};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Normal,
    Edit,
}

pub(crate) struct App {
    pub(crate) state: JsonEditorState,
    pub(crate) path: Option<PathBuf>,
    pub(crate) config: Config,
    pub(crate) dirty: bool,
    pub(crate) popup: bool,
    pub(crate) theme: Theme,
    pub(crate) textarea: TextArea<'static>,
    pub(crate) mode: Mode,
    pub(crate) message: Option<String>,
    pub(crate) view_height: usize,
    pub(crate) edit_row: usize,
    pub(crate) edit_col: usize,
    pub(crate) input_rect: Rect,
    pub(crate) tree_rect: Rect,
    pub(crate) scrollbar_rect: Rect,
    pub(crate) hbar_rect: Rect,
    pub(crate) scrollbar_interaction: ScrollBarInteraction,
    pub(crate) hbar_interaction: ScrollBarInteraction,
    pub(crate) menu: MenuState<Action>,
    pub(crate) menu_group: Option<usize>,
    pub(crate) menu_rect: Rect,
    clipboard: Clipboard,
    /// What the last copy action put on the clipboard, kept for the tests:
    /// a headless run has no clipboard to read back.
    #[cfg_attr(not(test), allow(dead_code))]
    copied: Option<String>,
}

impl App {
    pub(crate) fn new(
        state: JsonEditorState,
        path: Option<PathBuf>,
        config: Config,
        dirty: bool,
    ) -> Self {
        Self {
            state,
            path,
            config,
            dirty,
            popup: false,
            theme: Theme::default(),
            textarea: TextArea::default(),
            mode: Mode::Normal,
            message: None,
            view_height: 1,
            edit_row: 0,
            edit_col: 0,
            input_rect: Rect::default(),
            tree_rect: Rect::default(),
            scrollbar_rect: Rect::default(),
            hbar_rect: Rect::default(),
            scrollbar_interaction: ScrollBarInteraction::new(),
            hbar_interaction: ScrollBarInteraction::new(),
            menu: build_menu(),
            menu_group: None,
            menu_rect: Rect::default(),
            clipboard: Clipboard::new(),
            copied: None,
        }
    }

    /// Returns `true` when the application should quit.
    pub(crate) fn handle_key(&mut self, input: Input) -> bool {
        if self.popup {
            return self.popup_key(input);
        }
        // `MenuState::is_active` is not a usable gate: `tui-menu` keeps its
        // root highlighted forever, so it is always "active". Track openness
        // with the group whose dropdown is showing instead.
        if self.menu_group.is_some() {
            return self.menu_key(input);
        }
        match self.mode {
            Mode::Normal => self.normal_key(input),
            Mode::Edit => {
                self.edit_input(input);
                false
            }
        }
    }

    /// Mouse: the scrollbars own their areas (clicks, arrows, thumb drags);
    /// clicks select in the tree (key vs value by position) and in the input
    /// line (which places the text cursor); the wheel scrolls.
    /// Returns `true` when the mouse event asks the app to quit, so a click on
    /// a menu button that quits ends the session just as the key does.
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) -> bool {
        // The scrollbars only see events that land on them. Offering a click
        // anywhere to the scrollbar would make a click on the track scroll,
        // which is not what a click in the tree should ever do.
        if let Some(event) = scroll_event(mouse) {
            if inside(self.scrollbar_rect, mouse) {
                let vbar = scrollbar(
                    self.state.line_count(),
                    self.tree_rect.height as usize,
                    self.state.scroll(),
                );
                if let Some(ScrollCommand::SetOffset(offset)) =
                    vbar.handle_event(self.scrollbar_rect, event, &mut self.scrollbar_interaction)
                {
                    self.state.set_scroll(offset);
                    return false;
                }
            }
            if inside(self.hbar_rect, mouse) {
                let hbar = h_scrollbar(
                    self.state.content_width(),
                    self.tree_rect.width as usize,
                    self.state.scroll_x(),
                );
                if let Some(ScrollCommand::SetOffset(offset)) =
                    hbar.handle_event(self.hbar_rect, event, &mut self.hbar_interaction)
                {
                    self.state.set_scroll_x(offset);
                    return false;
                }
            }
        }

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if inside(self.menu_rect, mouse) {
                    self.click_menu_title(mouse);
                } else if let Some(index) = self.click_menu_item(mouse) {
                    // The dropdown floats above the panels, so its rows win
                    // over whatever lies beneath. Reopen the group first so
                    // the highlight starts on its first row, then walk down:
                    // the crate's own navigation can leave it anywhere.
                    if let Some(group) = self.menu_group {
                        self.open_menu(group);
                        for _ in 0..index {
                            self.menu.down();
                        }
                        self.menu.select();
                    }
                    if self.menu_actions() {
                        return true;
                    }
                } else {
                    if self.menu_group.is_some() {
                        self.menu.reset();
                        self.menu_group = None;
                    }
                    if inside(self.tree_rect, mouse) {
                        let row = self.state.scroll() + (mouse.row - self.tree_rect.y) as usize;
                        let col =
                            (mouse.column - self.tree_rect.x) as usize + self.state.scroll_x();
                        // What was under the cursor before this click. A
                        // hidden block opens on a second click, like clicking
                        // a folder twice, so a click that moves the selection
                        // away only selects.
                        let was_on = (
                            self.state.cursor_path().to_vec(),
                            self.state.selected_field(),
                        );
                        if self.state.select_at(row, col) {
                            self.state.ensure_cursor_visible(self.view_height);
                            self.state
                                .ensure_cursor_visible_x(self.tree_rect.width as usize);
                            if self.state.selected_field() == Field::Value
                                && self.state.is_collapsed()
                                && was_on == (self.state.cursor_path().to_vec(), Field::Value)
                            {
                                self.state.expand_block();
                                self.message = Some("block shown".to_string());
                            }
                        }
                    } else if inside(self.input_rect, mouse) {
                        self.place_text_cursor(mouse);
                    }
                }
            }
            MouseEventKind::ScrollDown if inside(self.tree_rect, mouse) => {
                let top = self.state.scroll() + 3;
                self.state.set_scroll(top);
            }
            MouseEventKind::ScrollUp if inside(self.tree_rect, mouse) => {
                let top = self.state.scroll().saturating_sub(3);
                self.state.set_scroll(top);
            }
            MouseEventKind::ScrollDown if inside(self.hbar_rect, mouse) => {
                let x = self.state.scroll_x() + 3;
                self.state.set_scroll_x(x);
            }
            MouseEventKind::ScrollUp if inside(self.hbar_rect, mouse) => {
                let x = self.state.scroll_x().saturating_sub(3);
                self.state.set_scroll_x(x);
            }
            MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                if inside(self.tree_rect, mouse) || inside(self.hbar_rect, mouse) =>
            {
                let x = if mouse.kind == MouseEventKind::ScrollRight {
                    self.state.scroll_x() + 3
                } else {
                    self.state.scroll_x().saturating_sub(3)
                };
                self.state.set_scroll_x(x);
            }
            _ => {}
        }
        false
    }

    /// The unsaved-changes popup: quit anyway, save and quit, or cancel.
    fn popup_key(&mut self, input: Input) -> bool {
        match input.key {
            Key::Char('q') => true,
            Key::Char('s') => {
                self.save();
                true
            }
            Key::Esc => {
                self.popup = false;
                false
            }
            _ => false,
        }
    }

    /// Quitting: with autosave the file is written on the way out; without it,
    /// unsaved changes ask first.
    fn request_exit(&mut self) -> bool {
        match self.path.as_ref() {
            None => true,
            Some(_) if self.config.autosave => {
                self.save();
                true
            }
            Some(_) if !self.dirty => true,
            Some(_) => {
                self.popup = true;
                false
            }
        }
    }

    /// Normal mode: the keymap decides what runs. An action bound to a key
    /// that no longer exists is simply never taken, so unbinding is done by
    /// pointing the action somewhere harmless.
    fn normal_key(&mut self, input: Input) -> bool {
        self.message = None;
        let Some(action) = self.action_for(&input) else {
            // Anything else starts editing, the way typing over a cell does.
            if let Key::Char(_) = input.key
                && !input.ctrl
                && !input.alt
                && !input.shift
            {
                self.begin_edit_fresh();
                self.textarea.input(input);
            }
            return false;
        };
        match action {
            Action::Quit | Action::Menu => {}
            _ => {
                self.state.ensure_cursor_visible(self.view_height);
                self.state
                    .ensure_cursor_visible_x(self.tree_rect.width as usize);
            }
        }
        if self.run(action) {
            return true;
        }
        false
    }

    /// The action an input event runs, if any. An action can be bound to
    /// several keys, so all of them are tried.
    fn action_for(&self, input: &Input) -> Option<Action> {
        Action::all().into_iter().find(|action| {
            self.config.bindings(*action).into_iter().any(|binding| {
                // `shift` is a property of the key rather than a modifier, so
                // it is matched separately: Tab is not Shift+Tab.
                binding.shift == input.shift && binding.matches(input)
            })
        })
    }

    /// Runs an action, returning `true` when the app should quit.
    fn run(&mut self, action: Action) -> bool {
        match action {
            Action::Quit => self.request_exit(),
            Action::Menu => {
                self.open_menu(0);
                false
            }
            Action::Save => {
                self.save();
                false
            }
            Action::Add => {
                self.do_add();
                false
            }
            Action::Delete => {
                self.do_delete();
                false
            }
            Action::Unflatten => {
                self.do_unflatten();
                false
            }
            Action::CopyEntry => {
                self.do_copy_entry();
                false
            }
            Action::CopyKey => {
                self.do_copy_key();
                false
            }
            Action::CopyValue => {
                self.do_copy_value();
                false
            }
            Action::CopySelected => {
                self.do_copy_selected();
                false
            }
            Action::ValueToString => {
                self.do_value_to_string();
                false
            }
            Action::StringToValue => {
                self.do_string_to_value();
                false
            }
            Action::Duplicate => {
                self.do_duplicate();
                false
            }
            Action::MoveUp => {
                self.do_move_up();
                false
            }
            Action::MoveDown => {
                self.do_move_down();
                false
            }
            Action::MoveAcrossUp => {
                self.do_move_across_up();
                false
            }
            Action::MoveAcrossDown => {
                self.do_move_across_down();
                false
            }
            Action::EditValue => {
                self.do_edit_value();
                false
            }
            Action::EditKey => {
                self.do_edit_key();
                false
            }
            Action::HideBlock => {
                self.do_hide_block();
                false
            }
            Action::ShowBlock => {
                self.do_show_block();
                false
            }
            Action::ToggleBlock => {
                self.do_toggle_block();
                false
            }
            Action::SelectUp => {
                self.state.select_up();
                false
            }
            Action::SelectDown => {
                self.state.select_down();
                false
            }
            Action::SelectLeft => {
                self.state.select_left();
                false
            }
            Action::SelectRight => {
                self.state.select_right();
                false
            }
            Action::SelectLineUp => {
                self.state.select_up();
                false
            }
            Action::SelectLineDown => {
                self.state.select_down();
                false
            }
            Action::SelectWordLeft => {
                self.state.select_left();
                false
            }
            Action::SelectWordRight => {
                self.state.select_right();
                false
            }
            Action::SelectFirst => {
                self.state.cursor_to_first_child();
                false
            }
            Action::SelectLast => {
                self.state.select_left();
                false
            }
            Action::PageUp => {
                let top = self.state.scroll().saturating_sub(self.view_height / 2);
                self.state.set_scroll(top);
                false
            }
            Action::PageDown => {
                let top = self.state.scroll() + self.view_height / 2;
                self.state.set_scroll(top);
                false
            }
            Action::EditField => {
                self.begin_edit();
                false
            }
        }
    }

    fn edit_input(&mut self, input: Input) {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Esc, ..) => self.cancel(),
            (Key::Enter | Key::Char('\n' | '\r'), false, false, false) => {
                self.commit_and_move(JsonEditorState::select_down);
            }
            (Key::Enter | Key::Char('\n' | '\r'), false, false, true) => {
                self.commit_and_move(JsonEditorState::select_up);
            }
            (Key::Tab, false, false, false) => {
                self.commit_and_move(JsonEditorState::select_right);
            }
            (Key::Tab, false, false, true) => {
                self.commit_and_move(JsonEditorState::select_left);
            }
            (Key::Enter | Key::Char('\n' | '\r'), ..) => self.textarea.insert_newline(),
            _ => {
                self.textarea.input(input);
            }
        }
    }

    /// Like moving between cells in a spreadsheet: commit what is typed, then
    /// move the selection. An invalid commit keeps the buffer open instead.
    fn commit_and_move(&mut self, move_selection: fn(&mut JsonEditorState) -> bool) {
        if let Err(err) = self.commit_edit() {
            self.message = Some(err.to_string());
            return;
        }
        self.dirty = true;
        move_selection(&mut self.state);
        self.state.ensure_cursor_visible(self.view_height);
        self.state
            .ensure_cursor_visible_x(self.tree_rect.width as usize);
        self.mode = Mode::Normal;
    }

    fn report(&mut self, result: Result<(), EditError>) {
        match result {
            Ok(()) => self.dirty = true,
            Err(err) => self.message = Some(err.to_string()),
        }
    }

    /// Adds an entry at the selection and starts naming it.
    fn do_add(&mut self) {
        if let Err(err) = self.state.add_entry() {
            self.message = Some(err.to_string());
        } else {
            self.dirty = true;
            self.state.select_key();
            self.begin_edit();
        }
    }

    /// Copies the selected entry into a new one beside it.
    fn do_duplicate(&mut self) {
        let result = self.state.duplicate_entry();
        self.report(result);
    }

    fn do_delete(&mut self) {
        let result = self.state.delete_entry();
        self.report(result);
    }

    /// Puts the selected entry on the clipboard as valid JSON. A property is
    /// written as an object holding it, so the text parses on its own; an
    /// entry with no key is just its value.
    fn do_copy_entry(&mut self) {
        self.copy(self.entry_json(), "entry");
    }

    /// Puts the selected key on the clipboard, plain text without quotes.
    fn do_copy_key(&mut self) {
        let Some(key) = self.key_text() else {
            self.message = Some("only a property has a key to copy".to_string());
            return;
        };
        self.copy(key, "key");
    }

    /// Puts the selected value on the clipboard, as JSON text: a string keeps
    /// its quotes, a container its brackets.
    fn do_copy_value(&mut self) {
        self.copy(compact(self.state.selected()), "value");
    }

    /// Puts whatever the cursor is on — a key or a value — on the clipboard.
    fn do_copy_selected(&mut self) {
        match self.state.selected_field() {
            Field::Key => self.do_copy_key(),
            Field::Value => self.do_copy_value(),
        }
    }

    /// The selected entry as it would be written in a document: `"key":
    /// value` for a property, and the value alone when there is none.
    ///
    /// This is a JSON *entry*, not a standalone value — it is meant to be
    /// pasted into another object and be valid there without any editing. The
    /// key is quoted for that reason; nothing is wrapped around it.
    ///
    /// Whether a key or a value is selected makes no difference: this is the
    /// whole entry either way.
    fn entry_json(&self) -> String {
        let value = compact(self.state.selected());
        match self.state.key() {
            Some(key) => format!("{}: {value}", quote_string(&key)),
            None => value,
        }
    }

    /// The selected key, if the cursor is on one.
    fn key_text(&self) -> Option<String> {
        match self.state.selected_field() {
            Field::Key => Some(self.state.edit()),
            Field::Value => None,
        }
    }

    /// Puts text on the clipboard, saying what happened either way.
    fn copy(&mut self, text: String, what: &str) {
        self.copied = Some(text.clone());
        self.message = match self.clipboard.set_text(&text) {
            Ok(()) => Some(format!("copied the {what}: {text}")),
            Err(err) => Some(err),
        };
    }

    /// Turns the selected value into a string holding its JSON text.
    fn do_value_to_string(&mut self) {
        let result = self.state.value_to_string();
        self.report(result);
    }

    /// Turns a string into the value its text describes.
    fn do_string_to_value(&mut self) {
        let result = self.state.parse_value_text();
        self.report(result);
    }

    /// Removes the selected entry but moves its children up, so nothing inside
    /// it is lost.
    fn do_unflatten(&mut self) {
        let result = self.state.unflatten_entry();
        self.report(result);
    }

    fn do_move_up(&mut self) {
        let result = self.state.move_entry_up();
        self.report(result);
    }

    fn do_move_down(&mut self) {
        let result = self.state.move_entry_down();
        self.report(result);
    }

    /// Moves the entry across the line above it, which may take it into
    /// another object rather than just past a sibling.
    fn do_move_across_up(&mut self) {
        let result = self.state.move_entry_across_up();
        self.report(result);
    }

    fn do_move_across_down(&mut self) {
        let result = self.state.move_entry_across_down();
        self.report(result);
    }

    /// Hides the selected block. The document is untouched: only the tree gets
    /// shorter. A block that is already hidden stays hidden, so this never
    /// shows one — that is what `Show block` is for.
    fn do_hide_block(&mut self) {
        let hidden = self.state.is_collapsed();
        self.state.collapse_block();
        self.message = Some(if hidden {
            "block is already hidden".to_string()
        } else {
            "block hidden".to_string()
        });
    }

    /// Hides a block that is open and shows one that is hidden.
    fn do_toggle_block(&mut self) {
        if self.state.is_collapsed() {
            self.do_show_block();
        } else {
            self.do_hide_block();
        }
    }

    /// Shows the selected block again. Only the block under the cursor: the
    /// blocks nested inside it keep whatever state they were left in.
    fn do_show_block(&mut self) {
        let shown = !self.state.is_collapsed();
        self.state.expand_block();
        self.message = Some(if shown {
            "block is already shown".to_string()
        } else {
            "block shown".to_string()
        });
    }

    fn do_edit_value(&mut self) {
        self.state.select_value();
        self.begin_edit();
    }

    fn do_edit_key(&mut self) {
        if self.state.select_key() {
            self.begin_edit();
        } else {
            self.message = Some("only object entries have a key to edit".to_string());
        }
    }

    // -- menu -------------------------------------------------------------

    /// `tui-menu` only moves relative to the current highlight, so opening a
    /// group walks there from a reset state.
    fn open_menu(&mut self, index: usize) {
        self.menu.reset();
        self.menu.activate();
        for _ in 0..index {
            self.menu.right();
        }
        self.menu.select();
        self.menu_group = Some(index);
    }

    /// While the menu is open it owns the navigation keys.
    fn menu_key(&mut self, input: Input) -> bool {
        match input.key {
            Key::Esc => {
                self.menu.reset();
                self.menu_group = None;
            }
            Key::Enter => self.menu.select(),
            Key::Down | Key::Char('j') => self.menu.down(),
            Key::Up | Key::Char('k') => self.menu.up(),
            Key::Right | Key::Char('l') => {
                self.menu.right();
                self.menu_group = self
                    .menu_group
                    .map(|group| (group + 1).min(MENUS.len() - 1));
            }
            Key::Left | Key::Char('h') => {
                self.menu.left();
                self.menu_group = self.menu_group.map(|group| group.saturating_sub(1));
            }
            _ => {}
        }
        self.menu_actions()
    }

    /// A dropdown row of the open group. `tui-menu` hangs the dropdown under
    /// the group title with items one row below its top border.
    fn click_menu_item(&self, mouse: MouseEvent) -> Option<usize> {
        let group = self.menu_group?;
        let (_, items) = MENUS[group];
        let widest = items.iter().map(|(_, name)| name.len()).max()? as u16;
        let x = self.menu_rect.x + title_x(group) + 2;
        let y = self.menu_rect.y + 2;
        if mouse.column < x || mouse.column >= x + widest + 2 || mouse.row < y {
            return None;
        }
        let index = (mouse.row - y) as usize;
        (index < items.len()).then_some(index)
    }

    fn click_menu_title(&mut self, mouse: MouseEvent) {
        for (index, (name, _)) in MENUS.iter().enumerate() {
            let x = self.menu_rect.x + title_x(index);
            if mouse.column >= x && mouse.column < x + name.len() as u16 + 2 {
                self.open_menu(index);
                return;
            }
        }
    }

    /// Runs any picked menu buttons and closes the menu. Nothing else: with no
    /// button picked the menu stays open, so arrows can move around first.
    fn menu_actions(&mut self) -> bool {
        let actions: Vec<Action> = self
            .menu
            .drain_events()
            .map(|MenuEvent::Selected(action)| action)
            .collect();
        if actions.is_empty() {
            return false;
        }
        self.menu.reset();
        self.menu_group = None;
        let mut quit = false;
        for action in actions {
            quit |= self.run(action);
        }
        quit
    }

    /// Starts editing what is selected, like Excel's F2. A value arrives as
    /// the JSON text it is written as, and only what is inside its quotes or
    /// brackets is selected: the delimiters are what hold the type, so they
    /// stay put while the contents are changed.
    fn begin_edit(&mut self) {
        self.message = None;
        let (text, inside) = self.edit_buffer();
        self.textarea = TextArea::from(text.split('\n'));
        self.textarea.set_tab_length(TAB_LEN as u8);
        match inside {
            Some((first, last)) => {
                self.textarea.move_cursor(CursorMove::Jump(0, first as u16));
                self.textarea.start_selection();
                self.textarea.move_cursor(CursorMove::Jump(last.0, last.1));
            }
            None => self.textarea.select_all(),
        }
        self.edit_row = 0;
        self.edit_col = 0;
        self.mode = Mode::Edit;
    }

    /// The text to edit, and the cursor range its delimiters leave free. A key
    /// is plain text, so all of it is the part to change; a value is JSON, and
    /// only the text between its opening and closing delimiter is.
    fn edit_buffer(&self) -> (String, Option<Inside>) {
        let text = self.state.edit();
        if self.state.selected_field() == Field::Key {
            return (text, None);
        }
        let inside = inside_delimiters(&text);
        (text, inside)
    }

    /// Starts editing with an empty text area, like typing over a cell in
    /// Excel: the typed text replaces the selected field.
    fn begin_edit_fresh(&mut self) {
        self.begin_edit();
        self.textarea = TextArea::default();
        self.textarea.set_tab_length(TAB_LEN as u8);
    }

    /// Commits the buffer to the selected field; the other is untouched.
    fn commit_edit(&mut self) -> Result<(), EditError> {
        let text = self.buffer_text();
        self.state.commit(&text)
    }

    fn buffer_text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn cancel(&mut self) {
        self.mode = Mode::Normal;
        self.message = None;
    }

    /// Writes the document to its file; without a file there is nothing to
    /// save to (the result goes to stdout on exit).
    fn save(&mut self) {
        self.message = match &self.path {
            Some(path) => match fs::write(path, format!("{}\n", pretty(self.state.root()))) {
                Ok(()) => {
                    self.dirty = false;
                    Some("saved".to_string())
                }
                Err(err) => Some(err.to_string()),
            },
            None => Some("no file to save to (the result goes to stdout on exit)".to_string()),
        };
    }

    /// Clicking the input line starts editing and puts the text cursor under
    /// the pointer.
    fn place_text_cursor(&mut self, mouse: MouseEvent) {
        if self.mode != Mode::Edit {
            self.begin_edit();
        }
        let row = (self.edit_row + (mouse.row - self.input_rect.y) as usize)
            .min(self.textarea.lines().len() - 1);
        let col = (mouse.column - self.input_rect.x) as usize + self.edit_col;
        let line = self.textarea.lines()[row].clone();
        self.textarea.cancel_selection();
        self.textarea
            .move_cursor(CursorMove::Jump(row as u16, char_at(&line, col) as u16));
    }
}

/// The part of a value a selection should cover: the text between its
/// delimiters, given as the column the selection starts on and the (row,
/// column) it ends at.
type Inside = (usize, (u16, u16));

/// The cursor range inside a value's delimiters. `None` when the text is not a
/// delimited value — a number, a key, a bare word — and so has no inside.
fn inside_delimiters(text: &str) -> Option<Inside> {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() < 2 {
        return None;
    }
    // The opening and closing have to match: a value is written with both.
    let matched = matches!(
        (chars[0], chars[chars.len() - 1]),
        ('"', '"') | ('{', '}') | ('[', ']')
    );
    if !matched {
        return None;
    }
    // Start just past the opening delimiter, and end on the closing one, so
    // the delimiters stay put and only the contents are selected.
    Some((1, (0, (chars.len() - 1) as u16)))
}

fn inside(rect: Rect, mouse: MouseEvent) -> bool {
    mouse.column >= rect.x
        && mouse.column < rect.x + rect.width
        && mouse.row >= rect.y
        && mouse.row < rect.y + rect.height
}

/// Converts a crossterm mouse event into the scrollbar's backend-agnostic
/// input, so no crossterm feature of `tui-scrollbar` is needed.
fn scroll_event(mouse: MouseEvent) -> Option<ScrollEvent> {
    let pointer = |kind| {
        ScrollEvent::Pointer(PointerEvent {
            column: mouse.column,
            row: mouse.row,
            kind,
            button: PointerButton::Primary,
        })
    };
    let event = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => pointer(PointerEventKind::Down),
        MouseEventKind::Drag(MouseButton::Left) => pointer(PointerEventKind::Drag),
        MouseEventKind::Up(MouseButton::Left) => pointer(PointerEventKind::Up),
        MouseEventKind::ScrollDown
        | MouseEventKind::ScrollUp
        | MouseEventKind::ScrollLeft
        | MouseEventKind::ScrollRight => {
            let (axis, delta) = match mouse.kind {
                MouseEventKind::ScrollRight => (ScrollAxis::Horizontal, 1),
                MouseEventKind::ScrollLeft => (ScrollAxis::Horizontal, -1),
                MouseEventKind::ScrollDown => (ScrollAxis::Vertical, 1),
                _ => (ScrollAxis::Vertical, -1),
            };
            ScrollEvent::ScrollWheel(ScrollWheel {
                axis,
                delta,
                column: mouse.column,
                row: mouse.row,
            })
        }
        _ => return None,
    };
    Some(event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Binding;
    use ratatui::crossterm::event::KeyModifiers;

    fn editor(src: &str) -> App {
        editor_with(src, Config::default())
    }

    fn editor_with(src: &str, config: Config) -> App {
        let state = JsonEditorState::parse(src).unwrap();
        let mut app = App::new(state, None, config, false);
        // Nothing has been drawn, so give the tree a size and a viewport to
        // work against; without them the view scrolls on every cursor move.
        app.tree_rect = Rect::new(0, 0, 60, 20);
        app.view_height = 20;
        app
    }

    fn key(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// What the last copy action put on the clipboard. A headless run has no
    /// clipboard to read back, so the app records what it copied.
    fn copied(app: &App) -> String {
        app.copied.clone().expect("something was copied")
    }

    fn ctrl_key_shift(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ctrl: true,
            alt: false,
            shift: true,
        }
    }

    fn ctrl_key(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ctrl: true,
            alt: false,
            shift: false,
        }
    }

    /// What a terminal sends for a capital letter: the character is already
    /// upper case, and Shift is reported as held.
    fn shift_key(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ctrl: false,
            alt: false,
            shift: true,
        }
    }

    fn root(app: &App) -> String {
        let mut out = String::new();
        app.state.root().write_compact(&mut out);
        out
    }

    #[test]
    fn move_keys_reorder_among_siblings() {
        let mut app = editor(r#"{"a":1,"b":2,"c":3}"#);
        app.handle_key(key('j'));
        app.handle_key(shift_key('J'));
        assert_eq!(root(&app), r#"{"b":2,"a":1,"c":3}"#);
        app.handle_key(shift_key('K'));
        assert_eq!(root(&app), r#"{"a":1,"b":2,"c":3}"#);
    }

    /// A long document, so there is room to scroll in either direction.
    fn long_editor() -> App {
        let mut src = String::from("{");
        for n in 0..40 {
            if n > 0 {
                src.push(',');
            }
            src.push_str(&format!("\"k{n}\": {n}"));
        }
        src.push('}');
        let mut app = editor(&src);
        app.tree_rect = Rect::new(0, 0, 60, 10);
        app.scrollbar_rect = Rect::new(60, 0, 1, 10);
        app.hbar_rect = Rect::new(0, 10, 60, 1);
        app
    }

    /// A click at absolute coordinates, for the panels themselves.
    fn click_absolute(app: &mut App, column: u16, row: u16) {
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn the_scrollbars_still_scroll_when_clicked() {
        let mut app = long_editor();
        assert_eq!(app.state.scroll(), 0);

        // The vertical bar's track: clicking below the thumb pages down.
        click_absolute(&mut app, 60, 9);
        assert!(app.state.scroll() > 0, "the track still scrolls");

        // Its arrow at the very top goes back.
        let mut app = long_editor();
        click_absolute(&mut app, 60, 9);
        let moved = app.state.scroll();
        assert!(moved > 0);
        click_absolute(&mut app, 60, 0);
        assert!(
            app.state.scroll() < moved,
            "the arrow at the top scrolls back up"
        );
    }

    /// A click on a menu button: the title on the bar, then the row below.
    /// The bar is one row tall and the dropdown hangs under it.
    fn click_menu(app: &mut App, group: usize, row: u16) -> bool {
        app.menu_rect = Rect::new(0, 0, 60, 1);
        let x = crate::menu::title_x(group);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x + 1,
            row: 0,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x + 3,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    #[test]
    fn every_menu_button_works_when_clicked() {
        // File > Save.
        let mut app = editor(r#"{"a": 1}"#);
        app.path = Some(std::path::PathBuf::from("/nonexistent/dir/x.json"));
        assert!(!click_menu(&mut app, 0, 2), "Save does not quit");
        assert!(app.message.is_some(), "Save reports what it did");

        // Edit > Add entry.
        let mut app = editor(r#"{"a": 1}"#);
        assert!(!click_menu(&mut app, 1, 2), "Add does not quit");
        assert!(app.mode == Mode::Edit, "Add starts naming the new entry");
    }

    #[test]
    fn clicking_quit_in_the_menu_quits() {
        // With no unsaved changes there is nothing to ask about, so the
        // button should end the session on the first click.
        let mut app = editor(r#"{"a": 1}"#);
        let quit = click_menu(&mut app, 0, 3);
        assert!(app.message.is_none() && !app.popup, "it really was Quit");
        assert!(quit, "Quit ends the session");

        // The same button with a file and changes asks first, like the key.
        let mut app = editor(r#"{"a": 1}"#);
        app.path = Some(std::path::PathBuf::from("/tmp/x.json"));
        app.dirty = true;
        assert!(!click_menu(&mut app, 0, 3), "it asks instead of quitting");
        assert!(app.popup, "the unsaved-changes popup is up");
    }

    #[test]
    fn a_click_never_scrolls_the_tree() {
        let mut app = long_editor();
        for row in 0..10 {
            let before = app.state.scroll();
            let selected = app.state.cursor_line();
            // Columns across the panel, well away from either scrollbar.
            for column in [0, 5, 20] {
                click_absolute(&mut app, column, row);
                assert_eq!(
                    app.state.scroll(),
                    before,
                    "a click at row {row} must not scroll"
                );
                assert!(
                    app.state.cursor_line() != selected || row == 0,
                    "a click at row {row} should select something"
                );
            }
        }
    }

/// Commits the edit, the way Enter does.
fn enter(app: &mut App) {
    app.handle_key(Input {
        key: Key::Enter,
        ctrl: false,
        alt: false,
        shift: false,
    });
}

/// Cancels the edit, the way Esc does.
fn esc() -> Input {
    Input {
        key: Key::Esc,
        ctrl: false,
        alt: false,
        shift: false,
    }
}

/// The buffer as it is shown while editing, and the text it starts with
/// selected.
fn editing(app: &App) -> (String, Option<String>) {
        let text = app.textarea.lines().join("\n");
        let range = app.textarea.selection_range();
        let selected = range.map(|((sr, sc), (er, ec))| {
            let lines = app.textarea.lines();
            if sr == er {
                lines[sr].chars().take(ec).skip(sc).collect()
            } else {
                text.clone()
            }
        });
        (text, selected)
    }

    #[test]
    fn a_string_is_edited_between_its_quotes() {
        // `e` edits the value.
        let mut app = editor(r#"{"s": "hello"}"#);
        app.handle_key(key('j'));
        app.handle_key(key('e'));
        assert!(app.mode == Mode::Edit);
        let (text, selected) = editing(&app);
        assert_eq!(text, r#""hello""#, "the quotes are shown");
        assert_eq!(selected.as_deref(), Some("hello"), "only the inside is selected");

        // Submitting it untouched leaves it a string, even though `hello` is
        // not a number: the quotes are what say so.
        enter(&mut app);
        assert_eq!(root(&app), r#"{"s":"hello"}"#, "still a string");

        // A string of digits is the case that matters: without the quotes it
        // would become a number.
        let mut app = editor(r#"{"s": "42"}"#);
        app.handle_key(key('j'));
        app.handle_key(key('e'));
        enter(&mut app);
        assert_eq!(root(&app), r#"{"s":"42"}"#, "not a number");
    }

    #[test]
    fn a_typed_string_still_becomes_one() {
        let mut app = editor(r#"{"s": ""}"#);
        app.handle_key(key('j'));
        app.handle_key(key('e'));
        // Replace the inside: the quotes stay put, so the value reads as a
        // string and not as the number 7.
        for c in "7".chars() {
            app.textarea.insert_str(c.to_string());
        }
        enter(&mut app);
        assert_eq!(root(&app), r#"{"s":"7"}"#, "a string, not the number 7");
    }

    #[test]
    fn non_string_values_are_still_edited_whole() {
        let mut app = editor(r#"{"n": 42, "o": {"a": 1}, "a": [1]}"#);
        app.handle_key(key('j'));
        app.handle_key(key('e'));
        let (text, selected) = editing(&app);
        assert_eq!(text, "42", "a number has no delimiters");
        assert_eq!(selected.as_deref(), Some("42"), "all of it is selected");

        // A container is shown whole, with only its inside selected.
        app.handle_key(esc());
        app.handle_key(key('j'));
        app.handle_key(key('e'));
        let (text, selected) = editing(&app);
        assert_eq!(text, r#"{"a":1}"#, "an object keeps its brackets");
        assert_eq!(selected.as_deref(), Some(r#""a":1"#), "only the inside is selected");

        app.handle_key(esc());
        app.handle_key(key('j'));
        app.handle_key(key('j')); // past the object's own entry
        app.handle_key(key('e'));
        let (text, selected) = editing(&app);
        assert_eq!(text, "[1]", "an array keeps its brackets");
        assert_eq!(selected.as_deref(), Some("1"), "only the inside is selected");
    }

    #[test]
    fn copying_an_entry_gives_an_entry_you_can_paste() {
        // The whole entry, whichever half of it the cursor is on.
        let mut app = editor(r#"{"key": [1, 2]}"#);
        app.handle_key(key('j'));
        for field in [Field::Key, Field::Value] {
            app.handle_key(if field == Field::Key {
                key('h')
            } else {
                key('l')
            });
            assert_eq!(app.state.selected_field(), field);
            app.handle_key(ctrl_key('c'));
            assert_eq!(
                copied(&app),
                r#""key": [1,2]"#,
                "the same entry with {field:?} selected"
            );
        }
        // No brackets around it: it is an entry, written as a document has it.
        assert!(
            !copied(&app).starts_with('{'),
            "an entry is not wrapped in an object"
        );
        // And pasting it into another document is valid with no editing.
        let pasted = format!("{{{}}}", copied(&app));
        let pasted = ratatui_json_editor::Json::parse(&pasted).expect("valid once pasted");
        assert_eq!(
            pasted,
            ratatui_json_editor::Json::parse(r#"{"key": [1, 2]}"#).unwrap(),
            "the entry came across whole"
        );

        // A value with characters that need quoting keeps them quoted.
        let mut app = editor(r#"{"a b": "x, y"}"#);
        app.handle_key(key('j'));
        app.handle_key(ctrl_key('c'));
        assert_eq!(
            copied(&app),
            r#""a b": "x, y""#,
            "both parts keep their quotes"
        );

        // Copying what is selected follows the cursor: the key on one field,
        // the value on the other.
        let mut app = editor(r#"{"key": [1, 2]}"#);
        app.handle_key(key('j'));
        app.handle_key(key('h'));
        app.handle_key(ctrl_key_shift('C'));
        assert_eq!(copied(&app), "key", "the key is under the cursor");
        app.handle_key(key('l'));
        app.handle_key(ctrl_key_shift('C'));
        assert_eq!(copied(&app), "[1,2]", "now the value is");

        // An entry with no key copies as its value alone.
        let mut app = editor("[1, 2]");
        app.handle_key(key('j'));
        app.handle_key(ctrl_key('c'));
        assert_eq!(copied(&app), "1");
    }

    #[test]
    fn copying_a_key_or_a_value_needs_a_configured_key() {
        // Neither is bound by default, so the letter keys start editing as
        // they always do.
        // `k` and `v` are taken by other actions — select left and nothing —
        // rather than by the copy buttons, so the document is untouched.
        let mut app = editor(r#"{"key": 1}"#);
        app.handle_key(key('j'));
        app.handle_key(key('k'));
        app.handle_key(key('v'));
        assert!(app.copied.is_none(), "nothing was copied");
        assert_eq!(root(&app), r#"{"key":1}"#, "the document is untouched");

        // The menu still runs them, and they put the right text there.
        // Under the Clipboard title: row 2 is Copy entry, row 3 Copy key.
        let mut app = editor(r#"{"key": 1}"#);
        app.handle_key(key('j'));
        app.handle_key(key('h'));
        assert!(!click_menu(&mut app, 2, 3), "Copy key does not quit");
        assert_eq!(copied(&app), "key", "the key, plain and unquoted");
    }

    #[test]
    fn copying_a_key_where_there_is_none_says_so() {
        // Run from the menu, since the action has no key of its own.
        let mut app = editor("[1, 2]");
        app.handle_key(key('j'));
        assert_eq!(
            app.state.selected_field(),
            Field::Value,
            "an array item has no key"
        );
        // Row 3 under the Clipboard title is Copy key.
        assert!(!click_menu(&mut app, 2, 3), "Copy key does not quit");
        assert!(app.copied.is_none(), "nothing was copied");
        assert!(app.message.is_some(), "the reason is shown");
    }

    #[test]
    fn ctrl_t_and_ctrl_y_convert_between_a_value_and_its_text() {
        let mut app = editor(r#"{"n": 42, "s": "hi"}"#);
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "n");

        app.handle_key(ctrl_key('t'));
        assert_eq!(
            root(&app),
            r#"{"n":"42","s":"hi"}"#,
            "Ctrl+T keeps the JSON text"
        );

        app.handle_key(ctrl_key('y'));
        assert_eq!(root(&app), r#"{"n":42,"s":"hi"}"#, "Ctrl+Y reads it back");

        // On a string the other way, it explains rather than doing nothing.
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [1], "s");
        app.handle_key(ctrl_key('t'));
        assert_eq!(
            root(&app),
            r#"{"n":42,"s":"hi"}"#,
            "the document is untouched"
        );
        assert!(app.message.is_some(), "the reason is shown");
    }

    #[test]
    fn capital_d_unflattens_the_selected_entry() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "a");
        app.handle_key(shift_key('D'));
        assert_eq!(
            root(&app),
            r#"{"x":1,"y":2,"b":3}"#,
            "D keeps what was inside; plain d would have deleted it"
        );

        // A node with nothing inside says so rather than doing a plain delete.
        let mut app = editor(r#"{"a": 1}"#);
        app.handle_key(key('j'));
        app.handle_key(shift_key('D'));
        assert_eq!(root(&app), r#"{"a":1}"#, "the document is left alone");
        assert!(app.message.is_some(), "the reason is shown");
    }

    #[test]
    fn ctrl_d_duplicates_the_selected_entry() {
        let mut app = editor(r#"{"a": 1, "b": 2}"#);
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "a");
        app.handle_key(ctrl_key('d'));
        assert_eq!(
            root(&app),
            r#"{"a":1,"a copy":1,"b":2}"#,
            "Ctrl+D copies it, and plain d still deletes"
        );

        // The copy is selected, so plain d would delete the copy.
        app.handle_key(key('d'));
        assert_eq!(root(&app), r#"{"a":1,"b":2}"#);
    }

    #[test]
    fn move_across_keys_change_the_parent() {
        // H and L move the entry into the neighbouring object, rather than
        // only past a sibling.
        let mut app = editor(r#"{"a": 1, "b": {"k": 0}, "c": 3}"#);
        app.handle_key(key('j'));
        app.handle_key(shift_key('L'));
        assert_eq!(root(&app), r#"{"b":{"a":1,"k":0},"c":3}"#);

        // Up into the object above: b is the first entry of nest, and the
        // line above nest is a, which can hold it.
        let mut app = editor(r#"{"a":{"k":0},"nest":{"b":1,"z":2}}"#);
        for _ in 0..4 {
            app.handle_key(key('j'));
        }
        assert_eq!(
            app.state.cursor_path(),
            [1, 0],
            "b, the first entry of nest"
        );
        app.handle_key(shift_key('H'));
        app.handle_key(shift_key('H'));
        assert_eq!(root(&app), r#"{"a":{"b":1,"k":0},"nest":{"z":2}}"#);
    }

    #[test]
    fn hide_and_show_keys_resize_the_tree_only() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        let before = app.state.line_count();
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "a");

        app.handle_key(key('-'));
        assert!(app.state.is_collapsed());
        assert_eq!(
            app.state.line_count(),
            before - 3,
            "a's entries and its close"
        );
        assert_eq!(
            root(&app),
            r#"{"a":{"x":1,"y":2},"b":3}"#,
            "the document keeps every entry"
        );

        // Hiding again is a no-op: the button only ever hides.
        app.handle_key(key('-'));
        assert!(app.state.is_collapsed(), "a second hide does not show it");
        assert_eq!(app.state.line_count(), before - 3);

        app.handle_key(shift_key('+'));
        assert!(!app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before);
        assert_eq!(root(&app), r#"{"a":{"x":1,"y":2},"b":3}"#);

        // Showing an open block is a no-op too.
        app.handle_key(shift_key('+'));
        assert_eq!(app.state.line_count(), before);
    }

    /// A click on the line the node at `path` renders on, at display
    /// `column`. The row comes from the state, so the test does not depend on
    /// how far the view has scrolled.
    fn click_path(app: &mut App, path: &[usize], column: u16) {
        let row = app.state.row_of(path).expect("the node has a line") as u16;
        let row = row.saturating_sub(app.state.scroll() as u16);
        click(app, row, column);
    }

    /// Moves the cursor to `path` with the select keys, so the tests do not
    /// depend on how many keys away a node is.
    fn focus(app: &mut App, path: &[usize]) {
        for _ in 0..8 {
            if app.state.cursor_path() == path {
                return;
            }
            app.handle_key(key('j'));
        }
        for _ in 0..8 {
            if app.state.cursor_path() == path {
                return;
            }
            app.handle_key(key('k'));
        }
        panic!(
            "could not reach {path:?}, ended at {:?}",
            app.state.cursor_path()
        );
    }

    fn click_cursor(app: &mut App, column: u16) {
        let row = app.state.cursor_line().saturating_sub(app.state.scroll()) as u16;
        click(app, row, column);
    }

    /// A click inside the tree, in display coordinates of its panel. The panel
    /// is set by hand because nothing has been drawn yet.
    fn click(app: &mut App, row: u16, column: u16) {
        app.tree_rect = Rect::new(0, 0, 60, 20);
        let tree = app.tree_rect;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: tree.x + column,
            row: tree.y + row,
            modifiers: KeyModifiers::NONE,
        });
    }

    #[test]
    fn clicking_a_hidden_block_opens_it() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        let before = app.state.line_count();
        // Row 1 is a's line; column 7 is its value, the block itself.
        click(&mut app, 1, 7);
        assert_eq!(app.state.cursor_path(), [0], "a is selected");
        assert_eq!(app.state.selected_field(), Field::Value);
        app.handle_key(key('-'));
        assert!(app.state.is_collapsed());
        assert_eq!(app.state.line_count(), 4, "a stands for its own block");

        // Hiding scrolled the view, so click where a is on screen now.
        click_cursor(&mut app, 7);
        assert!(!app.state.is_collapsed(), "the click opened the block");
        assert_eq!(app.state.line_count(), before);
        assert_eq!(root(&app), r#"{"a":{"x":1,"y":2},"b":3}"#);
    }

    #[test]
    fn a_hidden_block_needs_a_second_click_to_open() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        let before = app.state.line_count();
        focus(&mut app, &[0]);
        app.handle_key(key('-'));
        assert_eq!(app.state.collapsed_paths(), [vec![0usize]], "a is hidden");

        // A click somewhere else moves the selection and opens nothing.
        focus(&mut app, &[1]);
        click_cursor(&mut app, 7);
        assert_eq!(app.state.cursor_path(), [1], "b stays selected");
        assert_eq!(
            app.state.collapsed_paths(),
            [vec![0usize]],
            "a is still hidden: a click on b does not open it"
        );

        click_path(&mut app, &[0], 7);
        assert_eq!(app.state.cursor_path(), [0], "a is selected");
        assert_eq!(
            app.state.collapsed_paths(),
            [vec![0usize]],
            "the click that moved the selection did not open it"
        );

        // The second click, now that a is the selection, opens it.
        click_path(&mut app, &[0], 7);
        assert!(
            app.state.collapsed_paths().is_empty(),
            "the click on the selected block opens it"
        );
        assert_eq!(app.state.line_count(), before);
    }

    #[test]
    fn clicking_the_key_of_a_hidden_block_leaves_it_hidden() {
        let mut app = editor(r#"{"a": {"x": 1}, "b": 3}"#);
        app.handle_key(key('j'));
        app.handle_key(key('-'));
        assert!(app.state.is_collapsed());
        // With a's block hidden the view scrolls to keep a on screen.
        let screen = app.state.cursor_line().saturating_sub(app.state.scroll()) as u16;

        click(&mut app, screen, 2);
        assert_eq!(app.state.cursor_path(), [0], "a is selected again");
        assert_eq!(app.state.selected_field(), Field::Key);
        assert!(app.state.is_collapsed(), "the key is not the block");
    }

    #[test]
    fn the_toggle_key_hides_and_shows() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        let before = app.state.line_count();
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "a");

        app.handle_key(shift_key('*'));
        assert!(app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before - 3);

        app.handle_key(shift_key('*'));
        assert!(!app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before);
        assert_eq!(
            root(&app),
            r#"{"a":{"x":1,"y":2},"b":3}"#,
            "the document is untouched either way"
        );
    }

    #[test]
    fn a_rebound_key_moves_to_its_new_action() {
        // Delete is on `d` by default; move it to `x` and the old key starts
        // editing instead, the way any unbound letter would.
        let config = Config {
            binds: crate::config::Binds(vec![(
                Action::Delete,
                vec![Binding::from_json(Key::Char('x'), false, false, false)],
            )]),
            ..Config::default()
        };
        let mut app = editor_with(r#"{"a": 1, "b": 2}"#, config);
        app.handle_key(key('j'));
        app.handle_key(key('x'));
        assert_eq!(root(&app), r#"{"b":2}"#, "x deletes the selected entry");

        // `d` is unbound, so it starts editing, leaving the document alone.
        app.handle_key(key('d'));
        assert!(app.mode == Mode::Edit, "d starts editing now");
        assert_eq!(root(&app), r#"{"b":2}"#, "the document is untouched");
    }

    #[test]
    fn every_bind_of_an_action_runs_it() {
        for remove in ['x', 'z'] {
            let mut app = editor_with(r#"{"a": 1, "b": 2}"#, config_of());
            app.handle_key(key('j'));
            app.handle_key(key(remove));
            assert_eq!(root(&app), r#"{"b":2}"#, "{remove} deletes too");
        }
    }

    /// A second copy of a config, since the first is moved into the loop.
    fn config_of() -> Config {
        Config {
            binds: crate::config::Binds(vec![(
                Action::Delete,
                vec![
                    Binding::from_json(Key::Char('x'), false, false, false),
                    Binding::from_json(Key::Char('z'), false, false, false),
                ],
            )]),
            ..Config::default()
        }
    }

    #[test]
    fn a_refused_move_leaves_the_document_alone() {
        let mut app = editor(r#"{"a":1,"b":2}"#);
        for _ in 0..2 {
            app.handle_key(key('j'));
        }
        assert_eq!(app.state.cursor_path(), [1], "b, the document's last line");
        app.handle_key(shift_key('L'));
        assert_eq!(root(&app), r#"{"a":1,"b":2}"#, "there is nowhere to go");
        assert!(app.message.is_some(), "the reason is shown");
    }
}
