//! Screen 6: the funds held across the owner's investment accounts.
//!
//! A placeholder until the holdings model exists. It draws one line and owns
//! no state, so nothing above it has to know the screen is unfinished.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;

#[derive(Default)]
pub struct Funds;

impl Funds {
    pub fn new() -> Funds {
        Funds
    }

    pub fn render(&self, frame: &mut Frame, area: Rect) {
        frame.render_widget(Paragraph::new("No holdings yet."), area);
    }
}
