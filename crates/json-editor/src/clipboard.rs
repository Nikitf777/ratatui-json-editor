//! Copying to the system clipboard.
//!
//! `arboard` is created once and kept: putting text on the clipboard is a
//! request to the window system, and on some platforms the data only lives as
//! long as the process does. A failure is reported to the user rather than
//! swallowed, since "copied" is something they will come to rely on.

use arboard::Clipboard as Arboard;

/// The system clipboard, opened on first use.
pub(crate) struct Clipboard {
    inner: Option<Arboard>,
}

impl Clipboard {
    /// Opens the clipboard, remembering the failure instead of giving up: a
    /// session that cannot copy can still edit.
    pub(crate) fn new() -> Self {
        let inner = Arboard::new().ok();
        Self { inner }
    }

    /// Puts `text` on the clipboard.
    pub(crate) fn set_text(&mut self, text: &str) -> Result<(), String> {
        let Some(clipboard) = self.inner.as_mut() else {
            return Err("no clipboard is available here".to_string());
        };
        clipboard
            .set_text(text.to_string())
            .map_err(|err| err.to_string())
    }
}
