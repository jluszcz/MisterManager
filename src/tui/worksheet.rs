//! The allocation worksheet: one amount, one container's goals, and a live
//! remaining counter.
//!
//! Scoped to **one container** — it posts one amount to one account's goals.
//! Payday means running it twice, once per container, which is
//! what Planning already instructs and what physically happens: two separate
//! transfers. It is also what makes the remaining counter meaningful, since it
//! reconciles against that one container's excess.

use super::cursor::{Cursor, Scroll};
use super::form::{Field, parse_date};
use super::search::{Search, SearchBox};
use super::{Account, Label};
use crate::calc;
use crate::db::account::InterestPolicy;
use crate::db::goal::BatchKind;
use crate::db::{AccountId, GoalId};
use crate::money::Cents;
use anyhow::{Result, ensure};
use chrono::NaiveDate;

/// What the next keystroke edits.
///
/// Digits mean "the pot" or "this line" depending on this, which is why the
/// worksheet has a focus at all: the spec's key table describes `Lines`, and
/// the pot has to be editable for an interest posting.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Focus {
    Amount,
    Date,
    Lines,
}

impl Focus {
    const ORDER: [Focus; 3] = [Focus::Amount, Focus::Date, Focus::Lines];
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorksheetLine {
    pub goal_id: GoalId,
    pub name: String,
    pub amount: Cents,
    pub selected: bool,
    /// What this line was prefilled with, kept as `w`'s weight after the
    /// amount itself has been edited or zeroed. The per-paycheck ask on a
    /// payday sheet, the policy's share on an interest one.
    weight: Cents,
    /// Whether a digit has been typed since this amount was last set. A
    /// prefill is not "typed", so the first digit clears it instead of
    /// appending to it.
    typed: bool,
}

/// A committed worksheet, ready for `goal::insert_allocations`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub date: NaiveDate,
    pub shares: Vec<(GoalId, Cents)>,
}

/// Append a digit to a whole-dollar odometer.
///
/// Capped so a held-down key cannot overflow: a hundred million dollars is
/// already three orders of magnitude past anything this app will see.
fn push_digit(amount: Cents, digit: i64) -> Cents {
    let dollars = amount.dollars();
    if dollars >= 100_000_000 {
        return amount;
    }
    Cents::from_dollars(dollars * 10 + digit)
}

fn pop_digit(amount: Cents) -> Cents {
    Cents::from_dollars(amount.dollars() / 10)
}

/// The shares an interest posting opens with.
///
/// `eligible` is the container's open, `interest_eligible` goals and their
/// balances; `previous` is its last interest batch. Both are `(GoalId, Cents)`
/// so this stays a pure function over numbers.
///
/// **Eligibility is membership, and the policy only divides.** `previous` is
/// filtered to `eligible` before it can weight anything, so a goal that has
/// since closed or been made ineligible drops out under either policy --
/// otherwise the flag would say nothing on the one container that copies its
/// last posting instead of computing a fresh split. Filtering can empty it,
/// which is the no-history case reached by another route: the fallback below
/// catches both.
///
/// Brokerage is `pro_rata` and Rainy Day is `manual`: Brokerage's split is stable
/// and worth computing, Rainy Day's is a judgment call that shifts month to month,
/// and encoding that as a policy would rebuild the workbook's hand-edited
/// `IF(TRUE, …)` toggle in Rust. Prefilling from the last posting gives the
/// ergonomics of a policy with none of the maintenance.
///
/// A `manual` container with no previous batch falls back to `pro_rata`: the
/// first posting has nothing to copy.
pub fn interest_prefill(
    policy: InterestPolicy,
    total: Cents,
    eligible: &[(GoalId, Cents)],
    previous: &[(GoalId, Cents)],
) -> Result<Vec<(GoalId, Cents)>> {
    let previous: Vec<(GoalId, Cents)> = previous
        .iter()
        .filter(|(id, _)| eligible.iter().any(|(open, _)| open == id))
        .copied()
        .collect();
    let weights = match policy {
        InterestPolicy::Manual if !previous.is_empty() => previous.as_slice(),
        _ => eligible,
    };
    let by_id: Vec<(i64, Cents)> = weights.iter().map(|(id, c)| (id.0, *c)).collect();
    Ok(calc::pro_rata(total, &by_id)?
        .into_iter()
        .map(|(id, share)| (GoalId(id), share))
        .collect())
}

pub struct Worksheet {
    kind: BatchKind,
    container: Account,
    amount: Cents,
    amount_typed: bool,
    date: Field,
    lines: Vec<WorksheetLine>,
    /// Indices into `lines` that survive the name filter.
    visible: Vec<usize>,
    cursor: Cursor,
    focus: Focus,
    search: SearchBox,
    pending_slash: bool,
}

impl Worksheet {
    /// `prefill` is this container's goals and the amount each opens with, in
    /// screen order. The pot defaults to their sum, so a payday worksheet
    /// opens at zero remaining; `set_amount` overrides it for interest.
    pub fn new(
        kind: BatchKind,
        container: Account,
        today: NaiveDate,
        prefill: Vec<(GoalId, String, Cents)>,
    ) -> Worksheet {
        let lines: Vec<WorksheetLine> = prefill
            .into_iter()
            .map(|(goal_id, name, amount)| WorksheetLine {
                goal_id,
                name,
                amount,
                selected: false,
                weight: amount,
                typed: false,
            })
            .collect();
        let amount = lines.iter().map(|l| l.amount).sum();
        let visible = (0..lines.len()).collect();
        Worksheet {
            kind,
            container,
            amount,
            amount_typed: false,
            date: Field::date(today),
            lines,
            visible,
            cursor: Cursor::new(),
            focus: Focus::Amount,
            search: SearchBox::new(),
            pending_slash: false,
        }
    }

    pub fn set_amount(&mut self, amount: Cents) {
        self.amount = amount;
        self.amount_typed = false;
    }

    /// Replace every line's amount with `shares`, zeroing the lines it does
    /// not mention. How the interest prefill overrides the per-paycheck one.
    pub fn set_lines(&mut self, shares: &[(GoalId, Cents)]) {
        for line in &mut self.lines {
            line.amount = shares
                .iter()
                .find(|(id, _)| *id == line.goal_id)
                .map(|(_, c)| *c)
                .unwrap_or(Cents::ZERO);
            line.weight = line.amount;
            line.typed = false;
        }
    }

    pub fn kind(&self) -> BatchKind {
        self.kind
    }

    pub fn container(&self) -> AccountId {
        self.container.id()
    }

    pub fn amount(&self) -> Cents {
        self.amount
    }

    pub fn allocated(&self) -> Cents {
        self.lines.iter().map(|l| l.amount).sum()
    }

    pub fn remaining(&self) -> Cents {
        self.amount - self.allocated()
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn next_focus(&mut self) {
        self.focus = super::form::next_in(&Focus::ORDER, self.focus, 1);
    }

    pub fn previous_focus(&mut self) {
        self.focus = super::form::next_in(&Focus::ORDER, self.focus, -1);
    }

    pub fn type_char(&mut self, c: char) {
        match self.focus {
            Focus::Date => self.date.push(c),
            Focus::Amount => {
                if let Some(digit) = c.to_digit(10) {
                    if !self.amount_typed {
                        self.amount = Cents::ZERO;
                        self.amount_typed = true;
                    }
                    self.amount = push_digit(self.amount, digit as i64);
                }
            }
            Focus::Lines => {
                if let Some(digit) = c.to_digit(10)
                    && let Some(line) = self.cursor_line_mut()
                {
                    if !line.typed {
                        line.amount = Cents::ZERO;
                        line.typed = true;
                    }
                    line.amount = push_digit(line.amount, digit as i64);
                }
            }
        }
    }

    /// Step the date by `days`, when the date is what has focus. What `←`/`→`
    /// mean on every date field in the app.
    ///
    /// Guarded on the focus rather than on the key: the other two focuses are
    /// digit fields, and an arrow pressed on one of them must not move the
    /// date this batch will be stamped with.
    pub fn step_date(&mut self, days: i64) {
        if self.focus == Focus::Date {
            self.date.step_date(days);
        }
    }

    pub fn backspace(&mut self) {
        match self.focus {
            Focus::Date => self.date.backspace(),
            Focus::Amount => {
                self.amount_typed = true;
                self.amount = pop_digit(self.amount);
            }
            Focus::Lines => {
                if let Some(line) = self.cursor_line_mut() {
                    line.typed = true;
                    line.amount = pop_digit(line.amount);
                }
            }
        }
    }

    fn cursor_line_mut(&mut self) -> Option<&mut WorksheetLine> {
        let index = *self.visible.get(self.cursor.index())?;
        self.lines.get_mut(index)
    }

    pub fn lines(&self) -> Vec<&WorksheetLine> {
        self.visible.iter().map(|i| &self.lines[*i]).collect()
    }

    pub fn date_text(&self) -> &str {
        self.date.value()
    }

    pub fn title(&self) -> Label {
        let kind = match self.kind {
            BatchKind::Paycheck => "Payday",
            BatchKind::Interest => "Interest",
            BatchKind::Adhoc => "Allocate",
            BatchKind::Import => "Import",
        };
        Label::plain(format!("{kind} — post {} to ", self.amount))
            .account(self.container.clone())
            .text(" · Tab field · Enter commit · Esc cancel")
    }

    /// Zero lines are dropped: a goal the user chose not to fund must not get
    /// a no-op row in the batch and in its own ledger forever after.
    pub fn commit(&self) -> Result<Commit> {
        ensure!(
            self.remaining() >= Cents::ZERO,
            "over-allocated by {}",
            -self.remaining()
        );
        let shares: Vec<(GoalId, Cents)> = self
            .lines
            .iter()
            .filter(|l| l.amount != Cents::ZERO)
            .map(|l| (l.goal_id, l.amount))
            .collect();
        ensure!(!shares.is_empty(), "nothing to allocate");
        Ok(Commit {
            date: parse_date(self.date.value())?,
            shares,
        })
    }

    /// The lines an operator acts on: the selection when there is one, and the
    /// cursor line otherwise. Indices into `lines`.
    ///
    /// Intersected with `visible`: a line selected under one filter must not
    /// stay targeted after the filter changes and hides it, or an operator run
    /// off-screen would move money the user cannot see.
    fn targeted(&self) -> Vec<usize> {
        let selected: Vec<usize> = self
            .visible
            .iter()
            .copied()
            .filter(|i| self.lines[*i].selected)
            .collect();
        if !selected.is_empty() {
            return selected;
        }
        self.visible
            .get(self.cursor.index())
            .copied()
            .into_iter()
            .collect()
    }

    pub fn selected_count(&self) -> usize {
        self.lines.iter().filter(|l| l.selected).count()
    }

    pub fn toggle_selection(&mut self) {
        if let Some(line) = self.cursor_line_mut() {
            line.selected = !line.selected;
        }
    }

    /// `*`: select what the filter left on screen. Selecting hidden lines
    /// would make `/N` act on goals the user cannot see.
    pub fn select_all_visible(&mut self) {
        for index in self.visible.clone() {
            self.lines[index].selected = true;
        }
    }

    pub fn clear_selection(&mut self) {
        for line in &mut self.lines {
            line.selected = false;
        }
    }

    /// `z`: clear every visible line the operators are *not* targeting.
    ///
    /// The one operator that reads as the complement of the selection, and it
    /// is what gives a tick its meaning: the ticked lines are who this
    /// posting funds, so clearing the rest hands the whole pot to `s` and `w`
    /// to divide. The selection survives, which is what lets one tick carry
    /// through to the split that follows.
    ///
    /// Bounded by `visible` for the reason [`Worksheet::targeted`] is: a line
    /// the filter hides is a line the user cannot see, and zeroing it would
    /// move money off screen.
    pub fn zero_untargeted(&mut self) {
        let targeted = self.targeted();
        for index in self.visible.clone() {
            if targeted.contains(&index) {
                continue;
            }
            self.lines[index].amount = Cents::ZERO;
            self.lines[index].typed = false;
        }
    }

    /// Set each targeted line to `amount / n`, floored to a whole dollar.
    ///
    /// Divides the **top amount**, not the remaining: three lines each given
    /// `/3` then sum to the pot, where dividing the remaining would give
    /// 1/3, 2/9, 4/27 and never converge.
    pub fn divide(&mut self, n: i64) -> Result<()> {
        let share = super::share_of(self.amount, n)?;
        for index in self.targeted() {
            self.lines[index].amount = share;
            self.lines[index].typed = false;
        }
        Ok(())
    }

    /// Distribute what is left across the targeted lines, by largest
    /// remainder.
    ///
    /// `calc::pro_rata` over equal weights, so the leftover dollars go out one
    /// at a time instead of piling onto one goal -- every line here is a real
    /// allocation to a real goal, and a plug can drive one of them negative.
    pub fn spread(&mut self) -> Result<()> {
        self.spread_weighted(|_| Cents(1))
    }

    /// `w`: hand out what is left in the proportions the targeted lines were
    /// prefilled with.
    ///
    /// [`Worksheet::spread`] with the prefill's weights in place of equal
    /// ones, and additive for the same reason: both divide the *remaining*
    /// and leave every other line alone. What makes it worth a key of its own
    /// is that the weights outlive the amounts -- `z` zeroes a line's amount
    /// but not what the policy said it was worth -- so ticking a subset and
    /// pressing `w` reproduces the opening split over just those lines.
    ///
    /// Targeted weights that are all zero fall through to `calc::pro_rata`'s
    /// own answer for a basis of nothing: the whole remainder lands on the
    /// first of them, in a figure on screen and editable before `Enter`.
    pub fn spread_by_weight(&mut self) -> Result<()> {
        self.spread_weighted(|line| line.weight)
    }

    /// Both spreads: add the remaining to the targeted lines in the
    /// proportions `weight` gives them, by largest remainder.
    ///
    /// Nothing targeted is not a division by nothing -- it is a key pressed
    /// with no line under it, which leaves the sheet alone.
    fn spread_weighted(&mut self, weight: impl Fn(&WorksheetLine) -> Cents) -> Result<()> {
        let weights: Vec<(i64, Cents)> = self
            .targeted()
            .iter()
            .map(|i| (*i as i64, weight(&self.lines[*i])))
            .collect();
        if weights.is_empty() {
            return Ok(());
        }
        for (index, share) in calc::pro_rata(self.remaining(), &weights)? {
            let line = &mut self.lines[index as usize];
            line.amount += share;
            line.typed = false;
        }
        Ok(())
    }

    /// `/` waits for the next key: a digit runs [`Worksheet::divide`], and
    /// anything else begins a name filter. The spec gives `/` both jobs, and
    /// digits are already spoken for by line editing.
    pub fn slash(&mut self) {
        self.pending_slash = true;
    }

    pub fn is_pending_slash(&self) -> bool {
        self.pending_slash
    }

    pub fn cancel_pending_slash(&mut self) {
        self.pending_slash = false;
    }
}

impl Search for Worksheet {
    fn search_box(&self) -> &SearchBox {
        &self.search
    }

    fn search_box_mut(&mut self) -> &mut SearchBox {
        &mut self.search
    }

    /// The name filter matches a substring anywhere in the name, the same rule
    /// `savings::refilter` uses for the same `/` key on the Savings screen:
    /// typing `insur` finds `Car Insurance`.
    fn refilter(&mut self) {
        let needle = self.search().to_lowercase();
        self.visible = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| needle.is_empty() || l.name.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect();
        self.cursor.clamp(self.visible.len());
    }

    /// `/` is two keys on this screen, and opening the box spends the first:
    /// a filter that has begun is no longer waiting to become `/N`.
    fn begin_search(&mut self) {
        self.search.open();
        self.pending_slash = false;
    }
}

impl Scroll for Worksheet {
    fn cursor(&self) -> &Cursor {
        &self.cursor
    }

    fn cursor_mut(&mut self) -> &mut Cursor {
        &mut self.cursor
    }

    /// The lines the name filter left, not every goal in the container.
    fn row_count(&self) -> usize {
        self.visible.len()
    }
}

use super::form::centered;
use super::{amount, label_line, table_state};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line as TextLine;
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table};

/// Amount and date at the top, that container's goals below, a live remaining
/// counter. Returns the line viewport's height, for `PageUp`/`PageDown`.
pub fn render(frame: &mut Frame, sheet: &Worksheet) -> usize {
    let area = centered(
        frame.area(),
        76,
        frame.area().height.saturating_sub(4).max(8),
    );
    frame.render_widget(Clear, area);
    frame.render_widget(Block::bordered().title(label_line(&sheet.title())), area);
    let inner = area.inner(ratatui::layout::Margin::new(1, 1));

    let [header_area, lines_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    let caret = |focused: bool| if focused { "▌" } else { "" };
    frame.render_widget(
        Paragraph::new(vec![
            TextLine::from(format!(
                "      Amount  {}{}",
                sheet.amount(),
                caret(sheet.focus() == Focus::Amount)
            )),
            TextLine::from(format!(
                "        Date  {}{}",
                sheet.date_text(),
                caret(sheet.focus() == Focus::Date)
            )),
        ]),
        header_area,
    );

    let rows: Vec<Row> = sheet
        .lines()
        .iter()
        .map(|line| {
            Row::new(vec![
                Cell::from(if line.selected { "✓" } else { " " }),
                Cell::from(line.name.clone()),
                amount(line.amount),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(2),
        Constraint::Min(20),
        Constraint::Length(12),
    ];
    let height = usize::from(lines_area.height);
    let mut state = table_state(sheet.selected_index(), sheet.lines().len(), height);
    // The bar says the lines are what the next keystroke edits, so it belongs
    // to the focus and not to the cursor -- on an amount-focused worksheet it
    // reads as the first goal having focus, which the digits contradict. The
    // marker stays either way: the scroll keys move the cursor whatever has
    // focus, and its width is reserved on every row, so dropping it would
    // shift the whole table as focus moved.
    let lines_focused = sheet.focus() == Focus::Lines;
    frame.render_stateful_widget(
        Table::new(rows, widths)
            .row_highlight_style(if lines_focused {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            })
            .highlight_symbol("> "),
        lines_area,
        &mut state,
    );

    let status = if sheet.is_searching() {
        format!("/{}  · Enter to keep · Esc to clear", sheet.search())
    } else {
        format!(
            "remaining {}  ·  {} selected  ·  Space · * · - · z · /N · s · w",
            sheet.remaining(),
            sheet.selected_count()
        )
    };
    frame.render_widget(Paragraph::new(TextLine::from(status)), footer_area);

    height
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Group, Kind};

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn accounts() -> Vec<account::Account> {
        vec![
            account::Account {
                id: AccountId(1),
                code: "SAV".into(),
                name: "Rainy Day".into(),
                kind: Kind::Cash,
                sort: 0,
                group: Group::Savings,
                color: None,
            },
            account::Account {
                id: AccountId(2),
                code: "BKR".into(),
                name: "Brokerage".into(),
                kind: Kind::Cash,
                sort: 1,
                group: Group::Savings,
                color: None,
            },
            account::Account {
                id: AccountId(3),
                code: "NST".into(),
                name: "Nest Egg".into(),
                kind: Kind::Cash,
                sort: 2,
                group: Group::Savings,
                color: None,
            },
        ]
    }

    fn prefill() -> Vec<(GoalId, String, Cents)> {
        vec![
            (GoalId(1), "Bill Payments".to_string(), Cents(254_400)),
            (GoalId(2), "Housing".to_string(), Cents(69_300)),
            (GoalId(3), "Apple Watch".to_string(), Cents(1_000)),
        ]
    }

    fn sheet() -> Worksheet {
        Worksheet::new(
            BatchKind::Paycheck,
            Account::named(&accounts(), AccountId(1)),
            day(2026, 8, 14),
            prefill(),
        )
    }

    fn typed(sheet: &mut Worksheet, text: &str) {
        for c in text.chars() {
            sheet.type_char(c);
        }
    }

    /// The payday worksheet opens at zero remaining: its pot is the sum of
    /// what `per_paycheck` asked for.
    #[test]
    fn a_worksheet_opens_with_its_pot_equal_to_the_sum_of_its_lines() {
        let sheet = sheet();
        assert_eq!(sheet.amount(), Cents(324_700));
        assert_eq!(sheet.allocated(), Cents(324_700));
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// Interest opens on the container's excess instead, which is editable
    /// because the excess also carries any other unallocated remainder.
    #[test]
    fn setting_the_pot_moves_the_remaining_counter() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(500_000));
        assert_eq!(sheet.remaining(), Cents(175_300));
    }

    /// Digits build whole dollars. The first one clears whatever was there, so
    /// a prefilled line does not become 25445 when you type a 5 over 2544.
    #[test]
    fn typing_digits_on_a_line_replaces_the_prefill_then_appends() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Lines);

        typed(&mut sheet, "5");
        assert_eq!(sheet.lines()[0].amount, Cents(500));
        typed(&mut sheet, "00");
        assert_eq!(sheet.lines()[0].amount, Cents(50_000));
        sheet.backspace();
        assert_eq!(sheet.lines()[0].amount, Cents(5_000));
    }

    /// A prefilled line carries cents -- interest shares are not whole
    /// dollars -- and keeps them until a digit lands on it.
    #[test]
    fn a_prefill_keeps_its_cents_until_a_digit_is_typed_over_it() {
        let mut sheet = Worksheet::new(
            BatchKind::Interest,
            Account::named(&accounts(), AccountId(1)),
            day(2026, 7, 31),
            vec![(GoalId(1), "Emergency Savings".to_string(), Cents(130_048))],
        );
        sheet.next_focus();
        sheet.next_focus();
        assert_eq!(sheet.lines()[0].amount, Cents(130_048));
        typed(&mut sheet, "1600");
        assert_eq!(sheet.lines()[0].amount, Cents(160_000));
    }

    #[test]
    fn digits_edit_the_pot_while_the_amount_field_has_focus() {
        let mut sheet = sheet();
        assert_eq!(sheet.focus(), Focus::Amount);
        typed(&mut sheet, "8613");
        assert_eq!(sheet.amount(), Cents(861_300));
        assert_eq!(sheet.remaining(), Cents(861_300) - Cents(324_700));
    }

    /// `Tab` is the only way between the three, so digits never mean two
    /// things at once.
    #[test]
    fn tab_cycles_amount_date_and_lines_in_both_directions() {
        let mut sheet = sheet();
        assert_eq!(sheet.focus(), Focus::Amount);
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Date);
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Lines);
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Amount);
        sheet.previous_focus();
        assert_eq!(sheet.focus(), Focus::Lines);
    }

    #[test]
    fn the_date_field_is_typed_and_parsed_like_every_other_form() {
        let mut sheet = sheet();
        sheet.next_focus();
        for _ in 0..10 {
            sheet.backspace();
        }
        typed(&mut sheet, "2026-08-28");
        assert_eq!(sheet.commit().unwrap().date, day(2026, 8, 28));
    }

    /// The date steps a day at a time under `←`/`→`, as every date field in
    /// the app does. Typing still works: the arrows are the nudge, not the
    /// only way in.
    #[test]
    fn the_arrows_step_the_date_by_a_day() {
        let mut sheet = sheet();
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Date);
        sheet.step_date(1);
        assert_eq!(sheet.date_text(), "2026-08-15");
        sheet.step_date(-1);
        sheet.step_date(-1);
        assert_eq!(sheet.date_text(), "2026-08-13");
        assert_eq!(sheet.commit().unwrap().date, day(2026, 8, 13));
    }

    /// The pot and the lines are digit fields, and a stray arrow there must
    /// not move the date the commit is about to be stamped with.
    #[test]
    fn the_arrows_do_nothing_away_from_the_date_field() {
        let mut sheet = sheet();
        assert_eq!(sheet.focus(), Focus::Amount);
        sheet.step_date(1);
        sheet.next_focus();
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Lines);
        sheet.step_date(1);
        assert_eq!(sheet.date_text(), "2026-08-14");
    }

    /// A remainder simply stays unallocated and shows up in the container's
    /// excess, which is what happens in the sheet today.
    #[test]
    fn committing_with_a_positive_remainder_is_allowed() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(500_000));
        assert_eq!(sheet.remaining(), Cents(175_300));
        assert_eq!(sheet.commit().unwrap().shares.len(), 3);
    }

    /// Over-allocating would hand out money the container does not hold.
    #[test]
    fn committing_with_a_negative_remainder_is_refused() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(100_000));
        assert!(sheet.remaining() < Cents::ZERO);
        let err = sheet.commit().unwrap_err();
        assert!(err.to_string().contains("over-allocated"), "{err}");
    }

    /// A zero line is a goal the user chose not to fund. Writing it would put
    /// a no-op row in the batch, and in that goal's ledger forever, for every
    /// goal a worksheet leaves unfunded -- and a worksheet can cover many
    /// goals at once.
    #[test]
    fn a_zero_line_is_left_out_of_the_commit() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        typed(&mut sheet, "0");

        let shares = sheet.commit().unwrap().shares;
        assert_eq!(
            shares,
            vec![(GoalId(2), Cents(69_300)), (GoalId(3), Cents(1_000))]
        );
    }

    #[test]
    fn a_worksheet_with_nothing_allocated_is_refused() {
        let mut sheet = Worksheet::new(
            BatchKind::Adhoc,
            Account::named(&accounts(), AccountId(1)),
            day(2026, 8, 14),
            vec![(GoalId(1), "Bill Payments".to_string(), Cents::ZERO)],
        );
        sheet.set_amount(Cents(10_000));
        let err = sheet.commit().unwrap_err();
        assert!(err.to_string().contains("nothing"), "{err}");
    }

    #[test]
    fn the_title_names_the_amount_the_container_and_the_kind() {
        let sheet = sheet();
        let title = sheet.title();
        assert!(title.plain_text().contains("3,247.00"), "{title:?}");
        assert!(title.plain_text().contains("Rainy Day"), "{title:?}");
    }

    /// The container in the title is the same account the goal rows land in,
    /// so it is worth naming with the same color.
    #[test]
    fn the_worksheet_title_names_its_container_as_an_account() {
        let sheet = Worksheet::new(
            BatchKind::Paycheck,
            Account::named(&accounts(), AccountId(3)),
            day(2026, 8, 14),
            vec![(GoalId(1), "Bill Payments".to_string(), Cents(20_000))],
        );
        let title = sheet.title();
        assert!(
            title.plain_text().contains("post 200.00 to Nest Egg"),
            "{}",
            title.plain_text()
        );
        assert_eq!(title.accounts().len(), 1);
        assert_eq!(title.accounts()[0].id(), AccountId(3));
    }

    #[test]
    fn the_cursor_stops_at_both_ends_of_the_line_list() {
        let mut sheet = sheet();
        sheet.select_previous();
        assert_eq!(sheet.selected_index(), 0);
        for _ in 0..5 {
            sheet.select_next();
        }
        assert_eq!(sheet.selected_index(), 2);
    }

    fn amounts(sheet: &Worksheet) -> Vec<Cents> {
        sheet.lines().iter().map(|l| l.amount).collect()
    }

    /// `/N` divides the **top amount**, not the remaining. Three lines each
    /// typed `/3` then sum to the pot; dividing the remaining would give
    /// 1/3, 2/9, 4/27 and never converge.
    #[test]
    fn three_lines_each_divided_by_three_sum_to_the_pot() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(90_000));
        sheet.next_focus();
        sheet.next_focus();

        for _ in 0..3 {
            sheet.divide(3).unwrap();
            sheet.select_next();
        }

        assert_eq!(
            amounts(&sheet),
            vec![Cents(30_000), Cents(30_000), Cents(30_000)]
        );
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// Floored to whole dollars, which is the granularity the sheet allocates
    /// in -- so `/N` can leave a remainder, and that is fine.
    #[test]
    fn dividing_floors_to_whole_dollars() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(10_010));
        sheet.next_focus();
        sheet.next_focus();
        sheet.divide(3).unwrap();
        assert_eq!(sheet.lines()[0].amount, Cents(3_300));
    }

    #[test]
    fn dividing_by_zero_is_refused_rather_than_dividing() {
        let mut sheet = sheet();
        let err = sheet.divide(0).unwrap_err();
        assert!(err.to_string().contains("divide"), "{err}");
    }

    /// `s` hands the leftover dollars out by largest remainder rather than
    /// piling them on one goal, so no line can be driven negative.
    #[test]
    fn spreading_the_remaining_distributes_it_by_largest_remainder() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_all_visible();
        for line in sheet.lines() {
            assert!(line.selected);
        }
        sheet.divide(1_000_000).unwrap();
        assert_eq!(amounts(&sheet), vec![Cents::ZERO; 3]);

        sheet.set_amount(Cents(1_000));
        sheet.spread().unwrap();

        assert_eq!(sheet.remaining(), Cents::ZERO);
        assert_eq!(amounts(&sheet), vec![Cents(400), Cents(300), Cents(300)]);
    }

    /// A Brokerage-shaped interest prefill: the down-payment bucket is not
    /// eligible, so it opens at zero and weighs nothing.
    fn weighted_sheet() -> Worksheet {
        Worksheet::new(
            BatchKind::Interest,
            Account::named(&accounts(), AccountId(2)),
            day(2026, 8, 14),
            vec![
                (GoalId(1), "Home Down Payment".to_string(), Cents::ZERO),
                (GoalId(2), "Emergency Savings".to_string(), Cents(157_500)),
                (GoalId(3), "Mom & Dad".to_string(), Cents(37_000)),
            ],
        )
    }

    /// The flow `z` exists for: tick who this posting funds, free the pot,
    /// and divide it in the proportions the prefill opened with. The weight
    /// is the *opening* amount, so lines `z` has just zeroed still weigh what
    /// the policy said they were worth.
    #[test]
    fn spreading_by_weight_divides_the_remainder_in_the_proportions_the_lines_opened_with() {
        let mut sheet = weighted_sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.toggle_selection();
        sheet.zero_untargeted();
        assert_eq!(sheet.remaining(), Cents(194_500));

        sheet.clear_selection();
        sheet.select_next();
        sheet.toggle_selection();
        sheet.select_next();
        sheet.toggle_selection();
        sheet.spread_by_weight().unwrap();

        assert_eq!(
            amounts(&sheet),
            vec![Cents::ZERO, Cents(157_500), Cents(37_000)]
        );
        assert_eq!(sheet.remaining(), Cents::ZERO);
    }

    /// `w` is `s` with different weights, so it is additive too: it hands out
    /// the remaining and touches nothing else.
    #[test]
    fn spreading_by_weight_leaves_the_lines_it_does_not_target_alone() {
        let mut sheet = weighted_sheet();
        sheet.set_amount(Cents(195_500));
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_next();
        sheet.toggle_selection();

        sheet.spread_by_weight().unwrap();

        assert_eq!(
            amounts(&sheet),
            vec![Cents::ZERO, Cents(158_500), Cents(37_000)]
        );
    }

    /// The weights come from whichever prefill the sheet is showing: an
    /// interest posting overrides the per-paycheck asks it opened with, and
    /// `w` must divide by what is on screen rather than by what was.
    #[test]
    fn a_later_prefill_replaces_the_weights_it_opened_with() {
        let mut sheet = sheet();
        sheet.set_lines(&[(GoalId(2), Cents(100_000)), (GoalId(3), Cents(100_000))]);
        sheet.set_amount(Cents(300_000));
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_all_visible();

        sheet.spread_by_weight().unwrap();

        assert_eq!(
            amounts(&sheet),
            vec![Cents::ZERO, Cents(150_000), Cents(150_000)]
        );
    }

    /// `z` is what makes a tick mean "this posting funds these": it frees the
    /// pot by clearing every other line, so `s` and `w` have a remainder to
    /// divide. The selection itself is untouched, which is what lets the same
    /// tick carry through to the split that follows.
    #[test]
    fn zeroing_the_untargeted_lines_leaves_the_selection_holding_the_pot() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_next();
        sheet.toggle_selection();

        sheet.zero_untargeted();

        assert_eq!(
            amounts(&sheet),
            vec![Cents::ZERO, Cents(69_300), Cents::ZERO]
        );
        assert_eq!(sheet.selected_count(), 1);
    }

    /// The same rule every other operator obeys: a line the filter hides is
    /// not a line an operator may move.
    #[test]
    fn zeroing_the_untargeted_lines_spares_the_ones_the_filter_hides() {
        let mut sheet = sheet();
        sheet.begin_search();
        for c in "s".chars() {
            sheet.push_search(c);
        }
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Bill Payments", "Housing"]);
        sheet.end_search();
        sheet.toggle_selection();

        sheet.zero_untargeted();
        sheet.clear_search();

        assert_eq!(
            amounts(&sheet),
            vec![Cents(254_400), Cents::ZERO, Cents(1_000)],
            "Apple Watch was off screen"
        );
    }

    /// With nothing ticked it reads as "only this one", the same fallback
    /// `s` and `/N` make to the cursor line.
    #[test]
    fn zeroing_the_untargeted_lines_keeps_the_cursor_line_when_nothing_is_ticked() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_next();
        sheet.select_next();

        sheet.zero_untargeted();

        assert_eq!(
            amounts(&sheet),
            vec![Cents::ZERO, Cents::ZERO, Cents(1_000)]
        );
    }

    /// `s` adds to what is already there -- it distributes the *remaining*.
    #[test]
    fn spreading_adds_to_the_lines_rather_than_replacing_them() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(325_000));
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_all_visible();
        sheet.spread().unwrap();

        assert_eq!(sheet.remaining(), Cents::ZERO);
        assert_eq!(sheet.lines()[0].amount, Cents(254_400) + Cents(100));
    }

    #[test]
    fn spreading_a_negative_remaining_is_refused() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(1_000));
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_all_visible();
        assert!(sheet.spread().is_err());
    }

    /// Both operators act on the selection when there is one, and on the
    /// cursor line when there is not.
    #[test]
    fn an_operator_acts_on_the_cursor_line_when_nothing_is_selected() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(90_000));
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_next();
        sheet.divide(2).unwrap();

        assert_eq!(
            amounts(&sheet),
            vec![Cents(254_400), Cents(45_000), Cents(1_000)]
        );
    }

    #[test]
    fn an_operator_acts_on_every_selected_line_when_there_is_a_selection() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(90_000));
        sheet.next_focus();
        sheet.next_focus();
        sheet.toggle_selection();
        sheet.select_next();
        sheet.select_next();
        sheet.toggle_selection();
        assert_eq!(sheet.selected_count(), 2);

        sheet.divide(2).unwrap();
        assert_eq!(
            amounts(&sheet),
            vec![Cents(45_000), Cents(69_300), Cents(45_000)]
        );
    }

    #[test]
    fn clearing_the_selection_returns_the_operators_to_the_cursor_line() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_all_visible();
        assert_eq!(sheet.selected_count(), 3);
        sheet.clear_selection();
        assert_eq!(sheet.selected_count(), 0);
    }

    /// `/` waits for the next key: a digit is the fraction operator, anything
    /// else begins a name filter and is typed into it.
    #[test]
    fn a_slash_followed_by_a_letter_begins_a_filter() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.slash();
        assert!(sheet.is_pending_slash());

        sheet.cancel_pending_slash();
        sheet.begin_search();
        sheet.push_search('H');
        assert!(!sheet.is_pending_slash());
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Housing", "Apple Watch"]);
    }

    /// `*` selects what the filter left on screen, which is the whole point of
    /// filtering first.
    #[test]
    fn selecting_all_selects_only_the_visible_lines() {
        let mut sheet = sheet();
        sheet.begin_search();
        for c in "Hous".chars() {
            sheet.push_search(c);
        }
        sheet.select_all_visible();
        sheet.clear_search();

        let selected: Vec<bool> = sheet.lines().iter().map(|l| l.selected).collect();
        assert_eq!(selected, vec![false, true, false]);
    }

    /// A line selected under one filter must not stay targeted once a
    /// narrower filter hides it: running `/N` after switching filters must
    /// not move money on a goal the user can no longer see.
    #[test]
    fn narrowing_the_filter_after_selecting_a_line_drops_it_from_the_operators_targets() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.toggle_selection();
        assert_eq!(sheet.selected_count(), 1);

        sheet.begin_search();
        for c in "Hous".chars() {
            sheet.push_search(c);
        }
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Housing"], "Bill Payments must be filtered out");

        sheet.divide(2).unwrap();
        sheet.clear_search();

        assert_eq!(
            sheet.lines()[0].amount,
            Cents(254_400),
            "the hidden, previously-selected line must be untouched"
        );
        assert_eq!(
            sheet.lines()[1].amount,
            Cents(162_300),
            "the operator falls back to the visible cursor line instead"
        );
    }

    /// A filter that shrinks the list under the cursor must not leave it
    /// pointing past the end, where a digit would edit nothing.
    #[test]
    fn a_shrinking_filter_moves_the_cursor_into_bounds() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        sheet.select_next();
        sheet.select_next();
        assert_eq!(sheet.selected_index(), 2);

        sheet.begin_search();
        for c in "Hous".chars() {
            sheet.push_search(c);
        }
        assert_eq!(sheet.selected_index(), 0);
        sheet.type_char('5');
        assert_eq!(sheet.lines()[0].name, "Housing");
        assert_eq!(sheet.lines()[0].amount, Cents(500));
    }

    /// The filter matches a substring anywhere in the name, not just a
    /// prefix -- `ment` finds `Bill Payments` -- because it is the same rule
    /// `savings::refilter` already uses for the same `/` key.
    #[test]
    fn the_filter_matches_a_substring_anywhere_in_the_name() {
        let mut sheet = sheet();
        sheet.begin_search();
        for c in "ment".chars() {
            sheet.push_search(c);
        }
        let names: Vec<&str> = sheet.lines().iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["Bill Payments"]);
    }

    /// One container's interest posting. A third goal is excluded upstream by
    /// `interest_eligible`, which is why the denominator is these two alone.
    #[test]
    fn a_pro_rata_container_splits_across_its_eligible_goals_by_balance() {
        let shares = interest_prefill(
            InterestPolicy::ProRata,
            Cents::from_dollars(2000),
            &[
                (GoalId(1), Cents(10_600_195)),
                (GoalId(2), Cents(2_500_000)),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(
            shares,
            vec![
                (GoalId(1), Cents::from_dollars(1618)),
                (GoalId(2), Cents::from_dollars(382))
            ]
        );
    }

    /// Rainy Day's split is a judgment call that shifts month to month, so the
    /// prefill is last month's posting rescaled -- the ergonomics of a policy
    /// with none of the maintenance.
    #[test]
    fn a_manual_container_rescales_its_previous_posting_to_the_new_total() {
        let shares = interest_prefill(
            InterestPolicy::Manual,
            Cents::from_dollars(400),
            &[(GoalId(1), Cents(100)), (GoalId(2), Cents(100))],
            &[(GoalId(1), Cents(15_000)), (GoalId(2), Cents(5_000))],
        )
        .unwrap();
        assert_eq!(
            shares,
            vec![
                (GoalId(1), Cents::from_dollars(300)),
                (GoalId(2), Cents::from_dollars(100))
            ],
            "the previous posting's proportions, not the equal balances"
        );
    }

    /// A goal made ineligible must stop taking a share whatever the policy.
    /// `manual` copies the previous posting, so the copy is filtered too --
    /// otherwise the flag would mean nothing on the one container that uses
    /// that policy, which is where the owner is most likely to set it.
    #[test]
    fn a_manual_container_drops_a_previous_goal_that_is_no_longer_eligible() {
        let shares = interest_prefill(
            InterestPolicy::Manual,
            Cents::from_dollars(400),
            &[(GoalId(1), Cents(30_000))],
            &[(GoalId(1), Cents(15_000)), (GoalId(2), Cents(5_000))],
        )
        .unwrap();
        assert_eq!(shares, vec![(GoalId(1), Cents::from_dollars(400))]);
    }

    /// Every goal in the previous posting having since gone ineligible leaves
    /// nothing to copy -- the no-history case reached by another route.
    #[test]
    fn a_manual_container_whose_previous_goals_all_went_ineligible_falls_back_to_pro_rata() {
        let shares = interest_prefill(
            InterestPolicy::Manual,
            Cents::from_dollars(400),
            &[(GoalId(3), Cents(30_000)), (GoalId(4), Cents(10_000))],
            &[(GoalId(1), Cents(15_000)), (GoalId(2), Cents(5_000))],
        )
        .unwrap();
        assert_eq!(
            shares,
            vec![
                (GoalId(3), Cents::from_dollars(300)),
                (GoalId(4), Cents::from_dollars(100))
            ]
        );
    }

    /// The first ever Rainy Day posting has nothing to copy, and a container with
    /// no history must still open with something usable.
    #[test]
    fn a_manual_container_with_no_previous_posting_falls_back_to_pro_rata() {
        let shares = interest_prefill(
            InterestPolicy::Manual,
            Cents::from_dollars(400),
            &[(GoalId(1), Cents(30_000)), (GoalId(2), Cents(10_000))],
            &[],
        )
        .unwrap();
        assert_eq!(
            shares,
            vec![
                (GoalId(1), Cents::from_dollars(300)),
                (GoalId(2), Cents::from_dollars(100))
            ]
        );
    }

    /// Every share sums to the total, to the cent, whatever the policy --
    /// interest rows are not whole dollars.
    #[test]
    fn an_interest_prefill_always_sums_to_the_total() {
        let total = Cents(120_048);
        let shares = interest_prefill(
            InterestPolicy::ProRata,
            total,
            &[
                (GoalId(1), Cents(10_600_195)),
                (GoalId(2), Cents(2_500_000)),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(shares.iter().map(|(_, c)| *c).sum::<Cents>(), total);
    }

    fn drawn(sheet: &Worksheet) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(80, 12)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, sheet);
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn row_text(buffer: &ratatui::buffer::Buffer, y: u16) -> String {
        (0..buffer.area.width)
            .map(|x| buffer[(x, y)].symbol())
            .collect()
    }

    /// The row the cursor sits on. Nothing scrolls in these tests, so it is
    /// the one naming the first goal.
    fn cursor_row(buffer: &ratatui::buffer::Buffer) -> u16 {
        (0..buffer.area.height)
            .find(|y| row_text(buffer, *y).contains("Bill Payments"))
            .expect("no row names the cursor's goal")
    }

    fn is_reversed(buffer: &ratatui::buffer::Buffer, y: u16) -> bool {
        (0..buffer.area.width).any(|x| buffer[(x, y)].modifier.contains(Modifier::REVERSED))
    }

    /// The footer is where the operators are named without opening the panel,
    /// so it has to name all of them and still fit: a truncated line drops
    /// the last key it mentions, and the reader never learns the key exists.
    /// Sized at a seven-figure remaining, past anything this app will post.
    #[test]
    fn the_footer_names_every_operator_and_fits_the_modal() {
        let mut sheet = sheet();
        sheet.set_amount(Cents(123_456_789));
        let buffer = drawn(&sheet);
        let footer = (0..buffer.area.height)
            .map(|y| row_text(&buffer, y))
            .find(|row| row.contains("remaining"))
            .expect("no footer row is drawn");

        assert!(
            footer.contains(&format!("remaining {}", sheet.remaining())),
            "{footer}"
        );
        for key in ["Space", "*", "-", "z", "/N", "s", "w"] {
            assert!(footer.contains(key), "{key} is missing from {footer}");
        }
    }

    /// The reversed bar is what "the lines have focus" looks like, and the
    /// worksheet opens on the amount. Drawn there anyway, the first goal
    /// reads as what the next digit edits -- and the next digit goes to the
    /// pot.
    #[test]
    fn the_cursor_line_is_not_reversed_while_a_field_has_focus() {
        let sheet = sheet();
        assert_eq!(sheet.focus(), Focus::Amount);
        let buffer = drawn(&sheet);
        let y = cursor_row(&buffer);
        assert!(!is_reversed(&buffer, y), "{}", row_text(&buffer, y));
    }

    /// And it is back the moment the lines take focus, which is the only cue
    /// that digits now edit a line.
    #[test]
    fn the_cursor_line_reverses_once_the_lines_take_focus() {
        let mut sheet = sheet();
        sheet.next_focus();
        sheet.next_focus();
        assert_eq!(sheet.focus(), Focus::Lines);
        let buffer = drawn(&sheet);
        let y = cursor_row(&buffer);
        assert!(is_reversed(&buffer, y), "{}", row_text(&buffer, y));
    }

    /// The scroll keys move the line cursor whatever has focus, so the marker
    /// has to outlive the bar -- otherwise `↓` from the amount field moves
    /// something invisible. It also holds the table still: the symbol's width
    /// is reserved on every row, so dropping it would shift all three columns
    /// two cells as focus moved.
    #[test]
    fn the_cursor_marker_stays_on_the_line_whatever_has_focus() {
        let mut sheet = sheet();
        let unfocused = drawn(&sheet);
        sheet.next_focus();
        sheet.next_focus();
        let focused = drawn(&sheet);
        let row = |buffer: &ratatui::buffer::Buffer| {
            let y = cursor_row(buffer);
            row_text(buffer, y)
        };
        assert!(row(&unfocused).contains('>'), "{}", row(&unfocused));
        assert_eq!(row(&unfocused), row(&focused));
    }
}
