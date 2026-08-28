//! One goal's allocation history: every row its balance is the sum of, with
//! the two writes that correct one.
//!
//! Nothing else in the app reads an allocation back individually. They are
//! written by `a` on Savings, by every worksheet commit and by the import, and
//! before this the only way to reach one again was `U`, which deletes a whole
//! batch and only ever the most recent. A figure typed wrongly into a payday
//! four batches ago was fixed by posting an offsetting allocation beside it,
//! which leaves two rows where the owner meant one and makes the container's
//! history read as something that happened rather than as something that was
//! corrected.
//!
//! Named `History` and not `Allocations` because [`Modal::Allocation`] is
//! already the form that writes one, and two variants a letter apart would be
//! misread on sight.
//!
//! [`Modal::Allocation`]: super::modal::Modal::Allocation

use super::cursor::{Cursor, Viewport, impl_scroll};
use super::goal_form::AllocationForm;
use super::modal::Confirm;
use crate::db::goal::Allocation;
use crate::db::{AccountId, GoalId};
use crate::money::Cents;

/// Which of the three things the modal is doing.
///
/// The modes live inside the one type rather than as a second `Option<Modal>`
/// on `App`: nothing in the app opens a modal over a modal, and this does not
/// become the first thing that does. `Esc` then peels one layer at a time with
/// no flag anywhere saying what to return to.
pub(super) enum Mode {
    /// The rows, and the keys that act on the one under the cursor.
    List,
    /// `e`: the row under the cursor, in the same form `a` on Savings writes
    /// with. Which row it writes back to is [`AllocationForm::target`]'s to
    /// say, read off the same field the border is.
    Editing(AllocationForm),
    /// `d`: the last chance to back out, drawn through the same
    /// `form::render_fields` call the top-level dialog uses. `label` is the
    /// row as this list describes it.
    Confirming { action: Confirm, label: String },
}

pub struct History {
    goal_id: GoalId,
    goal_name: String,
    /// The container the goal sits in, and its name -- carried so `e` can
    /// build its form without going back to the Savings rows for either. The
    /// pot that form divides is *not* carried: it is read fresh when the form
    /// opens, the way `a`'s is.
    container: AccountId,
    container_name: String,
    rows: Vec<Allocation>,
    cursor: Cursor,
    mode: Mode,
}

impl History {
    /// The cursor opens on the **most recent** row. A correction is nearly
    /// always to a recent allocation, and a history long enough to scroll is
    /// one whose oldest row is the least likely thing being looked for.
    pub fn new(
        goal_id: GoalId,
        goal_name: &str,
        container: AccountId,
        container_name: &str,
        rows: Vec<Allocation>,
    ) -> History {
        let mut cursor = Cursor::new();
        cursor.last(rows.len());
        History {
            goal_id,
            goal_name: goal_name.to_string(),
            container,
            container_name: container_name.to_string(),
            rows,
            cursor,
            mode: Mode::List,
        }
    }

    pub fn goal_id(&self) -> GoalId {
        self.goal_id
    }

    pub fn goal_name(&self) -> &str {
        &self.goal_name
    }

    pub fn container(&self) -> AccountId {
        self.container
    }

    pub fn container_name(&self) -> &str {
        &self.container_name
    }

    pub fn rows(&self) -> &[Allocation] {
        &self.rows
    }

    pub fn selected(&self) -> Option<&Allocation> {
        self.rows.get(self.cursor.index())
    }

    /// The rows again after a write, with the cursor clamped onto whatever
    /// list is left -- a delete takes the last row away from under it.
    pub fn set_rows(&mut self, rows: Vec<Allocation>) {
        self.rows = rows;
        self.cursor.clamp(self.rows.len());
    }

    /// What the rows come to, which *is* the goal's balance: `goal::balance`
    /// is `SUM(cents)` over exactly these rows, so an audit visibly adds up to
    /// the figure the Savings screen shows.
    pub fn total(&self) -> Cents {
        self.rows
            .iter()
            .fold(Cents::ZERO, |sum, row| sum + row.cents)
    }

    pub(super) fn mode(&self) -> &Mode {
        &self.mode
    }

    pub(super) fn mode_mut(&mut self) -> &mut Mode {
        &mut self.mode
    }

    pub(super) fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// One row as a confirmation dialog names it: the date, what it moved,
    /// and whatever was said about it.
    pub fn label(&self, row: &Allocation) -> String {
        format!(
            "{}  {}  {}",
            row.date,
            crate::demo::figure(row.cents),
            crate::description::render(row.note.as_deref().unwrap_or_default())
        )
    }

    pub fn title(&self) -> String {
        format!(
            "{} — e edit · d delete · Esc close",
            crate::demo::text(&self.goal_name)
        )
    }
}

impl_scroll!(History, rows);

use super::form::centered;
use super::{Chrome, amount, render_table, right_header};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row};

/// One row per allocation, oldest first, footed by what they come to.
/// Returns the [`Viewport`] it drew: the height `PageUp`/`PageDown` move by,
/// and the row the next draw starts from.
pub(super) fn render(frame: &mut Frame, history: &History) -> Viewport {
    let area = centered(
        frame.area(),
        76,
        frame.area().height.saturating_sub(4).max(8),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::bordered().title(history.title()), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));

    let [rows_area, footer_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

    // An empty history draws neither a header nor columns: there is nothing
    // under them, and one full-width line says so where a placeholder squeezed
    // into a ten-wide `Date` column would be cut to `no allocat`.
    let empty = history.rows().is_empty();
    let viewport = match empty {
        true => render_table(
            frame,
            rows_area,
            history,
            Chrome::bare(),
            // `a` is named because it is the key that fills this list, and it
            // is deliberately not bound here.
            &[Constraint::Min(20)],
            vec![Row::new(vec![Cell::from(
                "no allocations yet · a on Savings adds one",
            )])],
            // Not a row the cursor may travel over, the way the Accounts
            // screen's placeholder is not.
            0,
        ),
        false => {
            let rows: Vec<Row> = history
                .rows()
                .iter()
                .map(|row| {
                    Row::new(vec![
                        Cell::from(row.date.to_string()),
                        // The same rule the ledger's Description column reads
                        // by, rather than a second one invented here: `—` for
                        // an absent note, and the text through the demo mask.
                        Cell::from(
                            crate::description::render(row.note.as_deref().unwrap_or_default())
                                .into_owned(),
                        ),
                        amount(row.cents),
                    ])
                })
                .collect();
            let header = Row::new(vec![
                Cell::from("Date"),
                Cell::from("Note"),
                right_header("Amount"),
            ])
            .style(Style::default().add_modifier(Modifier::BOLD));
            // The ledger's, minus the column this list does not have: `Note`
            // takes the single `Constraint::Min`, as `Description` does there.
            let widths = [
                Constraint::Length(10),
                Constraint::Min(20),
                Constraint::Length(14),
            ];
            render_table(
                frame,
                rows_area,
                history,
                Chrome::bare().header(header),
                &widths,
                rows,
                history.rows().len(),
            )
        }
    };

    if !empty {
        frame.render_widget(
            Paragraph::new(TextLine::from(format!(
                "Total {}",
                crate::demo::figure(history.total())
            ))),
            footer_area,
        );
    }

    viewport
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AllocationId;
    use crate::test_support::day;
    use crate::tui::MIN_WIDTH;
    use crate::tui::cursor::Scroll;
    use chrono::NaiveDate;

    fn row(id: i64, date: NaiveDate, cents: i64, note: Option<&str>) -> Allocation {
        Allocation {
            id: AllocationId(id),
            goal_id: GoalId(1),
            date,
            cents: Cents(cents),
            note: note.map(str::to_string),
        }
    }

    fn history() -> History {
        History::new(
            GoalId(1),
            "Vacation 2027",
            AccountId(2),
            "Rainy Day",
            vec![
                row(1, day(2026, 1, 15), 25_000, Some("opening")),
                row(2, day(2026, 3, 4), 40_000, None),
                row(3, day(2026, 5, 1), -10_000, Some("deposit refunded")),
            ],
        )
    }

    /// Every rendered line of the fixture, at the width the screens are laid
    /// out for.
    fn drawn(history: &History) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 14)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, history);
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..14)
            .map(|y| (0..MIN_WIDTH).map(|x| buffer[(x, y)].symbol()).collect())
            .collect()
    }

    /// A correction is nearly always to a recent allocation, and a history
    /// long enough to scroll is one whose oldest row is the least likely
    /// thing being looked for.
    #[test]
    fn the_cursor_opens_on_the_most_recent_row() {
        assert_eq!(
            history().selected().map(|row| row.id),
            Some(AllocationId(3))
        );
    }

    #[test]
    fn a_history_with_no_rows_selects_nothing() {
        let empty = History::new(GoalId(1), "Couch", AccountId(2), "Rainy Day", Vec::new());
        assert_eq!(empty.selected(), None);
    }

    /// The total *is* the goal's balance -- `goal::balance` is `SUM(cents)`
    /// over exactly these rows -- so an audit visibly adds up to the figure
    /// the Savings screen shows.
    #[test]
    fn the_total_is_the_sum_of_every_row_signs_included() {
        assert_eq!(history().total(), Cents(55_000));
    }

    /// A delete takes the last row out from under the cursor, so the rows
    /// arriving again have to bring it back inside the list.
    #[test]
    fn reloading_shorter_rows_clamps_the_cursor_onto_them() {
        let mut history = history();
        assert_eq!(history.selected_index(), 2);
        history.set_rows(vec![row(1, day(2026, 1, 15), 25_000, Some("opening"))]);
        assert_eq!(history.selected_index(), 0);
        assert_eq!(history.total(), Cents(25_000));
    }

    /// The widest plausible row at `MIN_WIDTH`: both fixed columns are whole,
    /// and the negative amount keeps its sign -- a right-aligned cell loses
    /// its *leading* characters when it is one column short, which for a
    /// figure is a wrong number rather than a visible ellipsis.
    #[test]
    fn the_fixed_columns_are_whole_at_the_minimum_width() {
        let widest = History::new(
            GoalId(1),
            "Vacation 2027",
            AccountId(2),
            "Rainy Day",
            vec![row(1, day(2026, 12, 31), -123_456_789, Some("a note"))],
        );
        let text = drawn(&widest).join("\n");
        assert!(text.contains("2026-12-31"), "{text}");
        assert!(text.contains("-1,234,567.89"), "{text}");
        assert!(text.contains("a note"), "{text}");
    }

    /// A note is `Option<String>` and draws through `description::render`,
    /// the same rule the ledger's Description column reads by.
    #[test]
    fn a_row_with_no_note_draws_an_em_dash() {
        let text = drawn(&history()).join("\n");
        assert!(text.contains("—"), "{text}");
        assert!(text.contains("opening"), "{text}");
    }

    #[test]
    fn the_rows_foot_with_their_total() {
        let text = drawn(&history()).join("\n");
        assert!(text.contains("Total 550.00"), "{text}");
    }

    /// A goal with no allocations says so rather than drawing an empty box --
    /// and names the key that fills the list, which is deliberately not one
    /// of this modal's own.
    #[test]
    fn a_goal_with_no_allocations_says_so_instead_of_footing_a_zero() {
        let empty = History::new(GoalId(1), "Couch", AccountId(2), "Rainy Day", Vec::new());
        let text = drawn(&empty).join("\n");
        assert!(text.contains("no allocations yet"), "{text}");
        assert!(!text.contains("Total"), "{text}");
    }

    /// The border is the one thing on screen naming the goal these rows
    /// belong to.
    #[test]
    fn the_border_names_the_goal_and_the_keys_that_act_on_a_row() {
        let text = drawn(&history()).join("\n");
        assert!(text.contains("Vacation 2027"), "{text}");
        assert!(text.contains("e edit"), "{text}");
        assert!(text.contains("d delete"), "{text}");
    }

    /// The goal name is the owner's own word for it, and this border is the
    /// one place a goal's history names it.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_goal_name_and_every_figure() {
        crate::demo::install_with_salt(7);
        let text = drawn(&history()).join("\n");

        assert!(
            !text.contains("Vacation 2027"),
            "the goal name survived: {text}"
        );
        assert!(
            text.contains(&crate::demo::text("Vacation 2027").to_string()),
            "no scrambled goal name found: {text}"
        );
        assert!(!text.contains("250.00"), "an amount survived: {text}");
        assert!(!text.contains("opening"), "a note survived: {text}");
        // An absence is not something to hide, and the dates are not figures.
        assert!(text.contains("—"), "{text}");
        assert!(text.contains("2026-01-15"), "{text}");
    }
}
