//! Screen 6's key handling.
//!
//! The screen holds no rows yet, so every key falls through to `App::dispatch`
//! above it.

use super::App;
use ratatui::crossterm::event::KeyEvent;

impl App {
    pub(super) fn funds_key(&mut self, _key: KeyEvent) -> anyhow::Result<()> {
        Ok(())
    }

    pub(super) fn reload_funds(&mut self) -> anyhow::Result<()> {
        Ok(())
    }
}
