//! The application state machine: modes, keys, mouse, and the actions that
//! change the document.

use std::fs;
use std::path::PathBuf;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui_json_editor::{EditError, JsonEditorState, Theme};
use ratatui_textarea::{CursorMove, Input, Key, TextArea};
use tui_menu::{MenuEvent, MenuState};
use tui_scrollbar::{
    PointerButton, PointerEvent, PointerEventKind, ScrollAxis, ScrollBarInteraction, ScrollCommand,
    ScrollEvent, ScrollWheel,
};

use crate::config::Config;
use crate::format::pretty;
use crate::menu::{build_menu, title_x, Action, MENUS};
use crate::ui::{char_at, h_scrollbar, scrollbar, TAB_LEN};

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
        if input.key == Key::F(10) {
            self.open_menu(0);
            return false;
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
    pub(crate) fn handle_mouse(&mut self, mouse: MouseEvent) {
        if let Some(event) = scroll_event(mouse) {
            let vbar = scrollbar(
                self.state.line_count(),
                self.tree_rect.height as usize,
                self.state.scroll(),
            );
            if let Some(ScrollCommand::SetOffset(offset)) =
                vbar.handle_event(self.scrollbar_rect, event, &mut self.scrollbar_interaction)
            {
                self.state.set_scroll(offset);
                return;
            }
            let hbar = h_scrollbar(
                self.state.content_width(),
                self.tree_rect.width as usize,
                self.state.scroll_x(),
            );
            if let Some(ScrollCommand::SetOffset(offset)) =
                hbar.handle_event(self.hbar_rect, event, &mut self.hbar_interaction)
            {
                self.state.set_scroll_x(offset);
                return;
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
                    self.menu_actions();
                } else {
                    if self.menu_group.is_some() {
                        self.menu.reset();
                        self.menu_group = None;
                    }
                    if inside(self.tree_rect, mouse) {
                        let row = self.state.scroll() + (mouse.row - self.tree_rect.y) as usize;
                        let col =
                            (mouse.column - self.tree_rect.x) as usize + self.state.scroll_x();
                        if self.state.select_at(row, col) {
                            self.state.ensure_cursor_visible(self.view_height);
                            self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
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

    fn normal_key(&mut self, input: Input) -> bool {
        self.message = None;
        match (input.key, input.ctrl, input.alt, input.shift) {
            (Key::Char('q') | Key::Esc, false, false, _) | (Key::Char('c'), true, _, _) => {
                return self.request_exit();
            }
            (Key::Char('s'), true, _, _) => {
                self.save();
                return false;
            }
            (Key::Char('j') | Key::Down, false, false, _) => {
                self.state.select_down();
            }
            (Key::Char('k') | Key::Up, false, false, _) => {
                self.state.select_up();
            }
            (Key::Char('h') | Key::Left, false, false, _) => {
                self.state.select_left();
            }
            (Key::Char('l') | Key::Right, false, false, _) => {
                self.state.select_right();
            }
            (Key::Tab, false, false, false) => {
                self.state.select_right();
            }
            (Key::Tab, false, false, true) => {
                self.state.select_left();
            }
            (Key::Enter, false, false, false) => {
                self.state.select_down();
            }
            (Key::Enter, false, false, true) => {
                self.state.select_up();
            }
            (Key::F(2), false, false, _) => {
                self.begin_edit();
            }
            (Key::Char('e'), false, false, _) => {
                self.do_edit_value();
            }
            (Key::Char('r'), false, false, _) => {
                self.do_edit_key();
            }
            (Key::Char('a'), false, false, _) => {
                self.do_add();
            }
            (Key::Char('d') | Key::Char('x') | Key::Delete, false, false, _) => {
                self.do_delete();
            }
            (Key::Char('J'), false, false, _) => {
                self.do_move_down();
            }
            (Key::Char('K'), false, false, _) => {
                self.do_move_up();
            }
            (Key::Char('-'), false, false, _) => {
                self.do_hide_block();
            }
            (Key::Char('+'), false, false, _) => {
                self.do_show_block();
            }
            (Key::Char('*'), false, false, _) => {
                self.do_toggle_block();
            }
            (Key::Char('H'), false, false, _) => {
                self.do_move_across_up();
            }
            (Key::Char('L'), false, false, _) => {
                self.do_move_across_down();
            }
            (Key::PageDown, false, false, _) => {
                let top = self.state.scroll() + self.view_height / 2;
                self.state.set_scroll(top);
            }
            (Key::PageUp, false, false, _) => {
                let top = self.state.scroll().saturating_sub(self.view_height / 2);
                self.state.set_scroll(top);
            }
            (Key::Char(_), false, false, _) => {
                self.begin_edit_fresh();
                self.textarea.input(input);
            }
            _ => return false,
        }
        self.state.ensure_cursor_visible(self.view_height);
        self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
        false
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
                self.commit_and_move(JsonEditorState::select_left);
            }
            (Key::Tab, false, false, true) => {
                self.commit_and_move(JsonEditorState::select_right);
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
        self.state.ensure_cursor_visible_x(self.tree_rect.width as usize);
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

    fn do_delete(&mut self) {
        let result = self.state.delete_entry();
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
                self.menu_group = self.menu_group.map(|group| (group + 1).min(MENUS.len() - 1));
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
            match action {
                Action::Save => self.save(),
                Action::Quit => quit = self.request_exit(),
                Action::Add => self.do_add(),
                Action::Delete => self.do_delete(),
                Action::MoveUp => self.do_move_up(),
                Action::MoveDown => self.do_move_down(),
                Action::MoveAcrossUp => self.do_move_across_up(),
                Action::MoveAcrossDown => self.do_move_across_down(),
                Action::EditValue => self.do_edit_value(),
                Action::EditKey => self.do_edit_key(),
                Action::HideBlock => self.do_hide_block(),
                Action::ShowBlock => self.do_show_block(),
                Action::ToggleBlock => self.do_toggle_block(),
            }
        }
        quit
    }

    /// Starts editing what is selected with the existing text selected, like
    /// Excel's F2: typing replaces it and the cursor sits at the end.
    fn begin_edit(&mut self) {
        self.message = None;
        let text = self.state.edit();
        self.textarea = TextArea::from(text.split('\n'));
        self.textarea.set_tab_length(TAB_LEN as u8);
        self.textarea.select_all();
        self.edit_row = 0;
        self.edit_col = 0;
        self.mode = Mode::Edit;
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

    fn editor(src: &str) -> App {
        let state = JsonEditorState::parse(src).unwrap();
        App::new(state, None, Config::default(), false)
    }

    fn key(c: char) -> Input {
        Input {
            key: Key::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
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
        app.handle_key(key('J'));
        assert_eq!(root(&app), r#"{"b":2,"a":1,"c":3}"#);
        app.handle_key(key('K'));
        assert_eq!(root(&app), r#"{"a":1,"b":2,"c":3}"#);
    }

    #[test]
    fn move_across_keys_change_the_parent() {
        // H and L move the entry into the neighbouring object, rather than
        // only past a sibling.
        let mut app = editor(r#"{"a": 1, "b": {"k": 0}, "c": 3}"#);
        app.handle_key(key('j'));
        app.handle_key(key('L'));
        assert_eq!(root(&app), r#"{"b":{"a":1,"k":0},"c":3}"#);

        // Up into the object above: b is the first entry of nest, and the
        // line above nest is a, which can hold it.
        let mut app = editor(r#"{"a":{"k":0},"nest":{"b":1,"z":2}}"#);
        for _ in 0..4 {
            app.handle_key(key('j'));
        }
        assert_eq!(app.state.cursor_path(), [1, 0], "b, the first entry of nest");
        app.handle_key(key('H'));
        app.handle_key(key('H'));
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
        assert_eq!(app.state.line_count(), before - 3, "a's entries and its close");
        assert_eq!(
            root(&app),
            r#"{"a":{"x":1,"y":2},"b":3}"#,
            "the document keeps every entry"
        );

        // Hiding again is a no-op: the button only ever hides.
        app.handle_key(key('-'));
        assert!(app.state.is_collapsed(), "a second hide does not show it");
        assert_eq!(app.state.line_count(), before - 3);

        app.handle_key(key('+'));
        assert!(!app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before);
        assert_eq!(root(&app), r#"{"a":{"x":1,"y":2},"b":3}"#);

        // Showing an open block is a no-op too.
        app.handle_key(key('+'));
        assert_eq!(app.state.line_count(), before);
    }

    #[test]
    fn the_toggle_key_hides_and_shows() {
        let mut app = editor(r#"{"a": {"x": 1, "y": 2}, "b": 3}"#);
        let before = app.state.line_count();
        app.handle_key(key('j'));
        assert_eq!(app.state.cursor_path(), [0], "a");

        app.handle_key(key('*'));
        assert!(app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before - 3);

        app.handle_key(key('*'));
        assert!(!app.state.is_collapsed());
        assert_eq!(app.state.line_count(), before);
        assert_eq!(
            root(&app),
            r#"{"a":{"x":1,"y":2},"b":3}"#,
            "the document is untouched either way"
        );
    }

    #[test]
    fn a_refused_move_leaves_the_document_alone() {
        let mut app = editor(r#"{"a":1,"b":2}"#);
        for _ in 0..2 {
            app.handle_key(key('j'));
        }
        assert_eq!(app.state.cursor_path(), [1], "b, the document's last line");
        app.handle_key(key('L'));
        assert_eq!(root(&app), r#"{"a":1,"b":2}"#, "there is nowhere to go");
        assert!(app.message.is_some(), "the reason is shown");
    }
}
