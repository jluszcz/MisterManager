//! The Planning screen: the waterfall, the constants and bills behind it,
//! the pin, and the payday `t` writes.
//!
//! The rows themselves are `crate::plan_rows`, which the report reads too --
//! what is here is only what a terminal adds to them: the cursor, the
//! editors, and the destination each line lands in.

use super::{App, DUPLICATE_SCAN_DAYS, NOTHING_SELECTED, WorksheetPrefill, add_share, destination};
use crate::db::setting::{self, key};
use crate::db::{GoalId, account, bill, goal};
use crate::money::Cents;
use crate::plan_line::{Destination, Line};
use crate::tui::cursor;
use crate::tui::form::ValueForm;
use crate::tui::modal::{Confirm, Modal};
use crate::tui::planning::{self as planning_screen, BillForm, Target, TransferConfirm};
use crate::tui::search::{self, Search};
use crate::{calc, plan, transfer};
use anyhow::{Result, bail};
use chrono::NaiveDate;
use ratatui::crossterm::event::{KeyCode, KeyEvent};

impl App {
    pub(super) fn planning_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.planning, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Char('e') => self.open_value_edit()?,
            KeyCode::Char('a') => self.modal = Some(Modal::Bill(BillForm::add())),
            KeyCode::Char('E') => self.open_bill_edit()?,
            KeyCode::Char('d') => self.open_bill_delete()?,
            KeyCode::Char('p') => self.pin()?,
            KeyCode::Char('P') => self.unpin()?,
            KeyCode::Char('t') => self.open_plan_transfers()?,
            KeyCode::Enter => self.open_plan_details(),
            _ => {}
        }
        Ok(())
    }

    /// `t` for the same reason the cash ledger's transfer is `t`: the key
    /// names the kind of action and not its size, and a letter meaning "move
    /// money between accounts" on one screen and something else on the next
    /// costs the owner more than the distinction would buy. What is particular
    /// to this one -- every row the plan calls for in a single transaction,
    /// and the allocation worksheets opened on top of them -- is said in its
    /// Help detail, where a reader who wants it will be.
    fn open_plan_transfers(&mut self) -> Result<()> {
        let settings = plan::settings_from_db(&self.db)?;
        let plan = plan::compute_from_db(&self.db, &settings, self.adhoc)?;
        let rows = match transfer::plan(&self.db, &plan.lines) {
            Ok(rows) => rows,
            Err(e) => {
                self.status = format!("{e:#}");
                return Ok(());
            }
        };
        let from = transfer::source(&self.db)?;
        let date = calc::business_day::add(self.today, 2)?;
        // Business days either side, not the one date the form opens on: the
        // date is editable before the write, so a run correcting a wrongly
        // dated first one steps off this default and onto the day that one
        // landed.
        let scanned = calc::business_day::window(date, DUPLICATE_SCAN_DAYS, DUPLICATE_SCAN_DAYS)?;
        let clashing = transfer::already_written(&self.db, from, &scanned, &rows)?;
        if !clashing.is_empty() {
            // A warning, not a block: these are ordinary ledger rows and a
            // second run with a corrected date is a real case. The dates are
            // named because they are days the form is not showing.
            let days: Vec<String> = clashing.iter().map(|d| d.to_string()).collect();
            let carry = if days.len() == 1 { "carries" } else { "carry" };
            self.status = format!("{} already {carry} matching rows", transfer::joined(&days));
        }
        self.modal = Some(Modal::PlanTransfers(TransferConfirm::new(
            rows, self.today, date,
        )));
        Ok(())
    }

    pub(super) fn commit_plan_transfers(&mut self) -> Result<()> {
        let Some(Modal::PlanTransfers(confirm)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected date keeps the form.
        let date = confirm.commit()?;
        let from = transfer::source(&self.db)?;
        let rows = confirm.rows().to_vec();
        // Built before the write: `worksheet_prefills` only reads goals and
        // settings, so this is behaviour-preserving, and it turns a
        // container-spanning plug -- the one case it can refuse -- into a
        // refusal before anything is written rather than an error stranded
        // after the payday is already on the ledger.
        let prefills = self.worksheet_prefills(date, &rows)?;
        transfer::execute(&self.db, from, date, &rows)?;
        let total: Cents = rows.iter().map(|r| r.cents()).sum();
        self.status = format!(
            "wrote {} transfers, {}",
            rows.len(),
            crate::demo::figure(total)
        );
        self.close_modal();
        self.reload()?;
        // Only stored once the reload has succeeded: a queue assigned before
        // a fallible step and abandoned by its `?` would sit non-empty with
        // no worksheet on screen, ready for some unrelated `Esc` or `Enter`
        // to resurrect it later.
        self.pending_worksheets = prefills;
        self.open_next_worksheet()
    }

    /// One prefilled worksheet per container a transfer landed in.
    ///
    /// Each container's lines are the goals its own Planning lines name, at
    /// those lines' amounts; the container holding the unclaimed goals also
    /// carries the plug, spread equally over them the way the worksheet's own
    /// `s` spreads.
    ///
    /// `date` is the transfer's, and each sheet opens on it: an allocation is
    /// the transfer read from the container's side, so a sheet left on today
    /// would credit the goals before the money reaches them -- and the owner
    /// may date a payday whenever they like.
    fn worksheet_prefills(
        &self,
        date: NaiveDate,
        rows: &[transfer::Row],
    ) -> Result<Vec<WorksheetPrefill>> {
        let spread_container = transfer::spread_container(&self.db)?;
        let spread = self.spread_asks()?;
        let mut out = Vec::new();
        for row in rows {
            let transfer::Row::Transfer {
                to, cents, lines, ..
            } = row
            else {
                continue;
            };
            let mut shares: Vec<(GoalId, Cents)> = Vec::new();
            for (line, amount) in lines {
                match line.destination() {
                    Destination::Goal(key) => {
                        if let Some(id) = setting::get(&self.db, key)? {
                            add_share(&mut shares, id, *amount);
                        }
                    }
                    Destination::Spread => {
                        if spread_container == Some(*to) && *amount != Cents::ZERO {
                            let asks: Vec<(i64, Cents)> = spread
                                .iter()
                                .filter(|(goal, _)| goal.container_account_id == *to)
                                .map(|(goal, ask)| (goal.id.0, *ask))
                                .collect();
                            // Asks that fit are met in full and the rest of
                            // the plug is left unallocated, on purpose: it is
                            // money to place by hand rather than money this
                            // has to find a home for.
                            for (id, share) in calc::fit(*amount, &asks)? {
                                add_share(&mut shares, GoalId(id), share);
                            }
                        }
                    }
                    Destination::Account(_) => {}
                }
            }
            // A row with no goal- or spread-backed line at all -- every line
            // landing in `to` is account-destination -- has nothing for a
            // worksheet to allocate; queuing one would open a container with
            // no goals and the whole pot sitting in `remaining`.
            if !shares.is_empty() {
                out.push((*to, date, *cents, shares));
            }
        }
        Ok(out)
    }

    /// `e` on Planning: a constant is typed into a one-field form, a
    /// destination is chosen from the goals that exist.
    ///
    /// `Planning::selected` only ever returns a row carrying one or the
    /// other, so the row's own values come out together with it or not at
    /// all.
    fn open_value_edit(&mut self) -> Result<()> {
        let opened = self
            .planning
            .selected()
            .map(|row| (row.editable, row.label.clone(), row.edit.clone()));
        match opened {
            None | Some((None, _, _)) => self.status = NOTHING_SELECTED.to_string(),
            Some((Some(planning_screen::Editable::Constant(target)), label, prefill)) => {
                // A percentage and a count of pay periods open the same
                // modal a target does, and only one of the three is money.
                // A bill's label arrives already masked: `tui::planning::build`'s
                // `bills` closure masks `plan_rows::Bill.label` before this row
                // is ever built, so a second mask here would scramble the
                // pseudonym itself rather than the name -- the double-mask
                // `src/transfer.rs`'s own `diagnose`/`spread_container` split
                // exists to avoid. Every other constant's label is the app's
                // own word for it and was never masked at all.
                let label = label.trim().to_string();
                let form = if target.is_money() {
                    ValueForm::money(label, &prefill)
                } else {
                    ValueForm::new(label, &prefill)
                };
                self.modal = Some(Modal::Value(target, form));
            }
            Some((Some(planning_screen::Editable::Destination(line)), _, _)) => {
                self.open_destination(line)?
            }
        }
        Ok(())
    }

    /// Why the transfers are unresolved, in full.
    ///
    /// The screen reports the failure in a cell about fifty columns wide,
    /// which is not enough to name the goal in the wrong container -- the one
    /// thing the owner needs in order to act on it. This is the same failure
    /// with room to explain itself.
    fn open_plan_details(&mut self) {
        let detail = self.planning.transfer_detail().to_vec();
        if detail.is_empty() {
            self.status = "the transfers resolve; nothing to explain".to_string();
            return;
        }
        self.modal = Some(Modal::Details("Transfers unresolved", detail));
    }

    /// What each goal the plug spreads over asks of this paycheck.
    ///
    /// The two dates the figure depends on are `App`'s, which is the whole of
    /// what this adds to [`transfer::spread_asks`]: the set and its pricing
    /// are that function's, so the Planning screen's coverage check and the
    /// prefill `t` writes cannot come to disagree about either.
    pub(super) fn spread_asks(&self) -> Result<Vec<(goal::Goal, Cents)>> {
        transfer::spread_asks(&self.db, self.today, self.periods_per_year()?)
    }

    /// The goals this line could point at, with the withdrawal among them.
    ///
    /// Every open goal, not the ones in some container the line already
    /// favours: which container a line lands in is a consequence of the goal
    /// chosen, not a constraint on choosing it.
    fn open_destination(&mut self, line: Line) -> Result<()> {
        let accounts = account::list(&self.db)?;
        let offer = |goal: goal::Goal| destination::Offered {
            container: accounts
                .iter()
                .find(|a| a.id == goal.container_account_id)
                .map_or("?", |a| a.name.as_str())
                .to_string(),
            id: goal.id,
            name: goal.name,
        };
        let offered = goal::all_with_balances(&self.db)?
            .into_iter()
            .map(|g| offer(g.goal))
            .collect();
        // Resolved through `goal::get` rather than looked up in `offered`:
        // `offered` holds open goals, and a line pointing at a goal that has
        // since been closed still points somewhere real. The list has to open
        // on it, or a stray `Enter` clears a destination nobody questioned.
        let current = match line.destination() {
            Destination::Goal(key) => match setting::get(&self.db, key)? {
                Some(id) => goal::get(&self.db, id)?.map(offer),
                None => None,
            },
            _ => None,
        };
        let suggestion = transfer::suggest(&self.db, line)?.map(offer);
        self.modal = Some(Modal::Destination(destination::Chooser::new(
            line, offered, current, suggestion,
        )));
        Ok(())
    }

    pub(super) fn destination_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(Modal::Destination(chooser)) = &mut self.modal
            && chooser.is_searching()
        {
            search::search_key(chooser, key);
            return Ok(());
        }
        if key.code == KeyCode::Enter {
            return self.commit_destination();
        }
        let Some(Modal::Destination(chooser)) = &mut self.modal else {
            return Ok(());
        };
        if cursor::scroll_key(chooser, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if !search::escape_kept_filter(chooser) {
                    self.close_modal();
                }
            }
            KeyCode::Char('/') => chooser.begin_search(),
            _ => {}
        }
        Ok(())
    }

    /// Write the chosen goal's **id** under this line's key, or clear it.
    ///
    /// The id, never the name: goal names are not unique, and the whole
    /// reason this is a list is that what the owner picked is a specific row
    /// rather than a string that might match three of them.
    fn commit_destination(&mut self) -> Result<()> {
        let Some(Modal::Destination(chooser)) = &self.modal else {
            return Ok(());
        };
        let line = chooser.line();
        let chosen = chooser.selected().cloned();
        let Destination::Goal(key) = line.destination() else {
            bail!("{} holds no goal to point", line.label());
        };
        let status = match chosen {
            None => return self.nothing_selected(),
            Some(destination::Choice::Unset) => {
                setting::clear(&self.db, key)?;
                format!("{} now leaves the tracked system", line.label())
            }
            Some(destination::Choice::Goal { id, name, .. }) => {
                setting::set(&self.db, key, id)?;
                format!("{} → {}", line.label(), crate::demo::text(&name))
            }
        };
        self.close_modal();
        self.status = status;
        self.reload()
    }

    pub(super) fn commit_value(&mut self) -> Result<()> {
        let Some(Modal::Value(target, form)) = &self.modal else {
            return Ok(());
        };
        // Parsed before the modal closes, so a rejected edit keeps the form
        // and everything typed into it.
        target.write(&self.db, self.today, form.value())?;
        self.status = format!("{} saved", form.label().plain_text().trim());
        self.close_modal();
        self.reload()
    }

    pub(super) fn commit_bill(&mut self) -> Result<()> {
        let Some(Modal::Bill(form)) = &self.modal else {
            return Ok(());
        };
        let edit = form.commit()?;
        match form.editing {
            Some(id) => bill::update(&self.db, id, &edit)?,
            None => {
                let sort = bill::next_sort(&self.db, edit.category)?;
                bill::insert(
                    &self.db,
                    &bill::NewBill {
                        label: edit.label.clone(),
                        cents: edit.cents,
                        category: edit.category,
                        sort,
                    },
                )?;
            }
        }
        self.status = format!(
            "{} {}",
            crate::demo::text(&edit.label),
            crate::demo::figure(edit.cents)
        );
        self.close_modal();
        self.reload()
    }

    /// A bill's label and category live nowhere else on the screen, so `e`'s
    /// one-field amount editor cannot reach them: `E` is the whole row. Editing
    /// in place also keeps the bill's `sort`, which deleting and re-adding
    /// would not.
    fn open_bill_edit(&mut self) -> Result<()> {
        let Some(Target::Bill(id)) = self.planning.selected_target() else {
            self.status = "only a bill has fields to edit".to_string();
            return Ok(());
        };
        let found = bill::get(&self.db, id)?;
        self.modal = Some(Modal::Bill(BillForm::edit(&found)));
        Ok(())
    }

    /// Only a bill can be deleted. Every other row is a constant, and the one
    /// constant that can be absent is unset with `p`.
    fn open_bill_delete(&mut self) -> Result<()> {
        let Some(Target::Bill(id)) = self.planning.selected_target() else {
            self.status = "only a bill can be deleted".to_string();
            return Ok(());
        };
        let found = bill::get(&self.db, id)?;
        let label = format!(
            "{}  {}",
            crate::demo::text(&found.label),
            crate::demo::figure(found.cents)
        );
        self.modal = Some(Modal::Confirm {
            action: Confirm::DeleteBill(id),
            label,
        });
        Ok(())
    }

    /// Re-run the waterfall and rebuild the screen.
    ///
    /// A database the waterfall cannot run against -- no Everyday checking account
    /// -- leaves the message on the Planning screen rather than failing the
    /// whole reload: every other screen still works there, and `App::new`
    /// must not refuse to start.
    pub(super) fn reload_planning(&mut self) -> Result<()> {
        match self.planning_view() {
            Ok(view) => self.planning.set_view(view),
            Err(err) => {
                self.planning.set_unavailable(format!("{err:#}"));
                Ok(())
            }
        }
    }

    /// Freeze `Excess (Actual)` at its whole-dollar floor.
    ///
    /// The floor is the same figure `compute` uses when nothing is pinned, so
    /// pinning a plan that is already balanced changes no number on screen --
    /// only whether it goes on moving.
    ///
    /// **Always pins, and overwrites a pin already there.** It does not
    /// toggle, because the press that follows a forgotten pin is the next
    /// payday's: `p` answering it with "unpinned" makes the press that
    /// matters the second one, every time. Re-pinning is also the only thing
    /// a second press could sensibly mean here -- the drift line exists to
    /// say a pin has gone stale, and the answer to a stale pin is a fresh
    /// one. Clearing is [`App::unpin`], on its own key.
    ///
    /// Both keys move together: a date with no amount would render a line
    /// about a plan that is not pinned, so `PINNED_AT` advances to today with
    /// the figure and the drift starts again from zero.
    ///
    /// Refused while the screen has no live view: `set_unavailable` leaves
    /// `excess_actual` holding whatever the last successful view left there,
    /// and pinning against that would freeze a number belonging to a plan the
    /// screen has just said it cannot compute.
    fn pin(&mut self) -> Result<()> {
        if self.planning.message().is_some() {
            self.status = "nothing to pin".to_string();
            return Ok(());
        }
        let was_pinned = self.planning.is_pinned();
        let pinned = self.planning.excess_actual().floor_to_dollar();
        setting::set(&self.db, key::PINNED_EXCESS, pinned)?;
        setting::set(&self.db, key::PINNED_AT, self.today)?;
        // Named apart so a press that replaced a pin does not read as one
        // that made the first: the figure below the plan has just changed
        // under the owner, and "pinned" alone would not say so.
        self.status = match was_pinned {
            true => format!("re-pinned {}", crate::demo::figure(pinned)),
            false => format!("pinned {}", crate::demo::figure(pinned)),
        };
        self.reload()
    }

    /// Put the waterfall back on the live balance.
    ///
    /// The other half of the payday the pin covers, and not an undo: the plan
    /// holds still while the legs are entered, and this is what ends that.
    /// Without it a pin is permanent, and `excess_used` would run off a
    /// frozen figure that never tracks reality again.
    ///
    /// Needs no live view, unlike [`App::pin`] -- it only clears two keys, and
    /// refusing here would strand a pin behind a footer still offering to
    /// remove it.
    fn unpin(&mut self) -> Result<()> {
        if !self.planning.is_pinned() {
            self.status = "nothing pinned".to_string();
            return Ok(());
        }
        setting::clear(&self.db, key::PINNED_EXCESS)?;
        setting::clear(&self.db, key::PINNED_AT)?;
        self.status = "unpinned".to_string();
        self.reload()
    }

    fn planning_view(&self) -> Result<planning_screen::View> {
        let settings = plan::settings_from_db(&self.db)?;
        let plan = plan::compute_from_db(&self.db, &settings, self.adhoc)?;
        // A misconfigured destination is reported on the screen, not thrown:
        // every figure above the transfer block is still right.
        let (transfers, transfer_error) = match transfer::plan(&self.db, &plan.lines) {
            Ok(rows) => (rows, None),
            Err(e) => (Vec::new(), Some(format!("{e:#}"))),
        };
        // The asks are read on their own, never chained to the call above.
        // The payday `Unmet Asks` exists for is the one where every line is
        // zero, and that is exactly the payday `transfer::plan` refuses with
        // `NOTHING_TO_TRANSFER` -- a read sharing its failure would go silent
        // on the one state it was put outside the block's `match` to reach.
        //
        // A failure of *this* read is its own answer, and is why it is not
        // propagated: the strict target reader is what a taxed goal with no
        // rate on record trips, and it would take the whole screen down over
        // an annotation the screen can simply omit. Zero draws no row, which
        // is what a gap nothing can measure should look like.
        let spread_ask_total = self
            .spread_asks()
            .map(|asks| asks.iter().map(|(_, ask)| *ask).sum())
            .unwrap_or(Cents::ZERO);
        // Copied out before the struct literal below moves `plan` into it.
        let plan_lines = plan.lines;
        Ok(planning_screen::View {
            plan,
            settings,
            wiring: transfer::wiring(&self.db)?,
            housing: bill::list(&self.db, bill::Category::Housing)?,
            other_bills: bill::list(&self.db, bill::Category::Other)?,
            pinned: setting::get(&self.db, key::PINNED_EXCESS)?,
            pinned_at: setting::get(&self.db, key::PINNED_AT)?,
            scrubbed_adhoc: (self.scrubbed_days() != 0).then_some(self.adhoc),
            transfers,
            spread_ask_total,
            transfer_error,
            transfer_detail: transfer::diagnose(&self.db, &plan_lines)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{self, Group, Kind};
    use crate::db::setting::{self, key};
    use crate::db::{AccountId, GoalId, goal};
    use crate::money::Cents;
    use crate::plan_line::{Destination, Line};
    use crate::rate::{BasisPoints, Percent};
    use crate::test_support::{day, walk_until};
    use crate::tui::MIN_WIDTH;
    use crate::tui::app::Screen;
    use crate::tui::app::test_support::*;
    use crate::tui::cursor::Scroll;
    use crate::tui::help::Topic;
    use crate::tui::modal::Modal;
    use crate::tui::planning::{self as planning_screen, Target};
    use crate::tui::search::Search;
    use crate::{db, plan, transfer};
    use chrono::{Datelike, NaiveDate};
    use ratatui::crossterm::event::KeyCode;

    /// The waterfall the screen is showing, run the way the screen runs it:
    /// the settings read once and handed to it, quoted at `app.adhoc`.
    fn computed_plan(app: &App, adhoc: NaiveDate) -> crate::calc::planning::Plan {
        plan::compute_from_db(&app.db, &plan::settings_from_db(&app.db).unwrap(), adhoc).unwrap()
    }

    fn planning_row<'a>(app: &'a App, label: &str) -> &'a crate::tui::planning::Row {
        app.planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == label)
            .unwrap_or_else(|| panic!("no Planning row labelled {label:?}"))
    }

    /// Three `Right`s off today (2026-08-15) puts Paycheck-Eve on the 18th,
    /// the day the fixture's Rent lands.
    fn scrub_past_the_rent(app: &mut App) {
        for _ in 0..3 {
            press(app, KeyCode::Right);
        }
        assert_eq!(app.adhoc, day(2026, 8, 18));
    }

    /// The scrub asks "what is the balance if the paycheck lands on a
    /// different day", and `Excess (Actual)` is that balance less Target and
    /// Buffer. Quoting it at the derived date while the Overview quotes the
    /// scrubbed one is two screens disagreeing about which day they mean.
    #[test]
    fn scrubbing_moves_the_date_planning_quotes_the_checking_balance_at() {
        let mut app = planning_app_with_a_row_after_today();
        let before = app.planning.excess_actual();

        scrub_past_the_rent(&mut app);

        assert_eq!(app.planning.excess_actual(), before - Cents(1_000_000));
    }

    /// `t` recomputes the plan rather than reading the one behind the screen,
    /// so it is its own chance to quote the wrong date -- and the one that
    /// would move real money to a figure the owner never saw.
    #[test]
    fn t_builds_its_transfers_from_the_scrubbed_plan() {
        let mut app = planning_app_with_a_row_after_today();
        scrub_past_the_rent(&mut app);
        let expected = computed_plan(&app, app.adhoc).lines.future_housing;

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        assert_eq!(modal_transfer_amount(&app, Line::FutureHousing), expected);
    }

    /// `t`'s confirmation is the one date field that opens neither on today
    /// nor blank: its rows are dated for when the transfers *land*, two
    /// business days out. The fixture's today is a Saturday, so this pins the
    /// weekend skip as well as the offset -- and pins the one opening date
    /// nothing else in the suite held down, which is how the invariant listing
    /// them came to be short by one.
    #[test]
    fn the_transfer_confirmation_opens_two_business_days_out() {
        let mut app = planning_app();
        assert_eq!(app.today.weekday(), chrono::Weekday::Sat);

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));

        let confirm = plan_transfers(&app);
        assert_eq!(confirm.date_value(), "2026-08-18");
        assert_eq!(confirm.commit().unwrap(), day(2026, 8, 18));
    }

    /// The confirmation's date is a text field like any other, and it has a
    /// key handler of its own -- the one place the editing keys could be
    /// missing while every form had them.
    #[test]
    fn the_transfer_confirmation_answers_the_editing_keys() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));
        ctrl_press(&mut app, 'u');
        type_str(&mut app, "2026-09-01");

        let confirm = plan_transfers(&app);
        assert_eq!(confirm.commit().unwrap(), day(2026, 9, 1));
    }

    /// What `t`'s confirmation modal says it will move for one line.
    fn modal_transfer_amount(app: &App, wanted: Line) -> Cents {
        let confirm = plan_transfers(app);
        confirm
            .rows()
            .iter()
            .find_map(|row| match row {
                transfer::Row::Transfer { lines, .. } => lines
                    .iter()
                    .find(|(line, _)| *line == wanted)
                    .map(|(_, cents)| *cents),
                transfer::Row::Withdrawal { line, cents } if *line == wanted => Some(*cents),
                transfer::Row::Withdrawal { .. } => None,
            })
            .unwrap_or_else(|| panic!("no transfer row carries {wanted:?}"))
    }

    /// Pinning freezes the figure the screen shows, which is now the scrubbed
    /// one: a pin that quoted the derived date would disagree with the row it
    /// was pressed on.
    #[test]
    fn p_pins_the_scrubbed_excess_the_screen_is_showing() {
        let mut app = planning_app_with_a_row_after_today();
        scrub_past_the_rent(&mut app);
        // Computed here rather than read off the screen: reading the screen
        // would agree with itself whichever date it had used.
        let expected = computed_plan(&app, app.adhoc)
            .excess_actual
            .floor_to_dollar();

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));

        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn five_opens_the_planning_screen_with_the_waterfall_on_it() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        assert_eq!(app.screen, Screen::Planning);
        assert_eq!(
            planning_row(&app, "Mortgage + HOA").value,
            Cents::from_dollars(1_500).to_whole_dollars()
        );
        assert!(app.planning.selected().unwrap().editable.is_some());
    }

    /// The write has to land in the database *and* be back on screen without
    /// another keystroke, which is what the reload after a commit is for.
    #[test]
    fn e_on_the_planning_screen_rewrites_the_constant_and_the_waterfall_moves() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.planning.selected_target(), Some(Target::Target));
        let before = planning_row(&app, "Excess (Actual)").value.clone();

        press(&mut app, KeyCode::Char('e'));
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "1000");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, key::PLANNING_TARGET).unwrap(),
            Some(Cents::from_dollars(1_000))
        );
        assert_eq!(planning_row(&app, "Target").value, "1,000");
        assert_ne!(
            planning_row(&app, "Excess (Actual)").value,
            before,
            "a lower target frees more excess"
        );
        assert_eq!(
            app.planning.selected_target(),
            Some(Target::Target),
            "the cursor stays where the edit was"
        );
    }

    /// The waterfall's own hand-typed figure: `e` on `Excess (Used)` pins
    /// what was typed, exactly as `p` pins what was computed, and every line
    /// below runs off it.
    #[test]
    fn e_on_excess_used_pins_the_figure_that_was_typed() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        for _ in 0..40 {
            if app.planning.selected_target() == Some(Target::PinnedExcess) {
                break;
            }
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.planning.selected_target(), Some(Target::PinnedExcess));
        assert!(!app.planning.is_pinned());

        press(&mut app, KeyCode::Char('e'));
        for _ in 0..16 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "1200");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(Cents::from_dollars(1_200))
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(app.today)
        );
        assert!(app.planning.is_pinned());
        assert_eq!(planning_row(&app, "Excess (Used)").value, "1,200");
    }

    /// A failed parse must keep the form open with what was typed, not
    /// discard the edit and leave the user guessing.
    #[test]
    fn a_rejected_edit_reports_it_and_keeps_the_form_open() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('e'));
        for _ in 0..12 {
            press(&mut app, KeyCode::Backspace);
        }
        type_str(&mut app, "lots");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_some(), "the form must survive a bad parse");
        assert!(app.status.contains("lots"), "{}", app.status);
        assert_eq!(setting::get(&app.db, key::PLANNING_TARGET).unwrap(), None);
    }

    #[test]
    fn a_adds_a_bill_and_the_biweekly_column_picks_it_up() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Char('a'));
        type_str(&mut app, "Newspaper");
        press(&mut app, KeyCode::Tab);
        type_str(&mut app, "30");
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Right);
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let added = planning_row(&app, "Newspaper");
        assert_eq!(added.value, "30");
        // 30 * 12 / 26 = 13.85, rounded up to a whole dollar.
        assert_eq!(added.extra, Cents::from_dollars(14).to_whole_dollars());
        assert_eq!(
            crate::db::bill::amounts(&app.db, crate::db::bill::Category::Other)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn d_then_y_deletes_the_selected_bill() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        walk_until!(
            matches!(app.planning.selected_target(), Some(Target::Bill(_))),
            press(&mut app, KeyCode::Down)
        );

        press(&mut app, KeyCode::Char('d'));
        press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none());
        assert_eq!(
            crate::db::bill::amounts(&app.db, crate::db::bill::Category::Housing)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(planning_row(&app, "Mortgage + HOA").value, "300");
    }

    #[test]
    fn capital_e_opens_the_selected_bill_prefilled() {
        use crate::tui::planning::BillField;

        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);

        press(&mut app, KeyCode::Char('E'));

        match &app.modal {
            Some(Modal::Bill(form)) => {
                assert_eq!(form.editing, Some(bill.id));
                assert_eq!(form.display(BillField::Label).plain_text(), bill.label);
                assert_eq!(
                    form.display(BillField::Amount).plain_text(),
                    bill.cents.to_string()
                );
                assert_eq!(form.category(), bill.category);
            }
            _ => panic!("no bill form is open"),
        }
    }

    /// `E` prefills the bill's own figure and its own label, so the form is
    /// where both would otherwise be published to whoever is watching -- the
    /// same rule every other amount and name field follows.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_amount_capital_e_opens_a_bill_on() {
        use crate::tui::planning::BillField;

        crate::demo::install_with_salt(7);
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);
        let real = bill.cents.to_string();

        press(&mut app, KeyCode::Char('E'));

        let Some(Modal::Bill(form)) = &app.modal else {
            panic!("no bill form is open");
        };
        let drawn = form.display(BillField::Amount).plain_text();
        assert_ne!(drawn, real);
        assert_eq!(drawn, crate::demo::typed(&real));
        let drawn_label = form.display(BillField::Label).plain_text();
        assert_ne!(drawn_label, bill.label);
        assert_eq!(drawn_label, crate::demo::text(&bill.label));
        // The buffer is untouched, so Enter still rewrites the real figure
        // and the real label.
        let committed = form.commit().unwrap();
        assert_eq!(committed.cents, bill.cents);
        assert_eq!(committed.label, bill.label);
    }

    /// `e` opens the same bill through a different path -- `open_value_edit`,
    /// which reads the row `tui::planning::build` already produced. Its
    /// `bills` closure masks `plan_rows::Bill.label` before `Row::bill` ever
    /// sees it, so `open_value_edit` must not mask a second time -- doing so
    /// would scramble the pseudonym instead of the name. The sweep cannot
    /// hold this shut on its own: `select_first_bill` always lands on
    /// `Mortgage`, which is excluded from the sweep's own check list as a
    /// heading collision, so only a direct assertion here does.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_label_lowercase_e_opens_a_bill_on() {
        crate::demo::install_with_salt(7);
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);
        assert_eq!(bill.label, "Mortgage");

        press(&mut app, KeyCode::Char('e'));

        let Some(Modal::Value(target, form)) = &app.modal else {
            panic!("no value form is open");
        };
        assert_eq!(*target, Target::Bill(bill.id));
        let drawn = form.label().plain_text();
        assert_ne!(drawn, "Mortgage");
        assert_eq!(drawn, crate::demo::text("Mortgage"));
        // The real label is untouched on record: this form only ever writes
        // the figure back, through `Target::write`.
        assert_eq!(
            crate::db::bill::get(&app.db, bill.id).unwrap().label,
            "Mortgage"
        );
    }

    /// `E` is the whole row, so committing it must rewrite the bill it opened
    /// on -- an insert would leave the old row behind and double the category.
    #[test]
    fn committing_capital_e_rewrites_the_bill_rather_than_adding_one() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let bill = select_first_bill(&mut app);
        let before = crate::db::bill::list(&app.db, bill.category).unwrap().len();

        press(&mut app, KeyCode::Char('E'));
        type_str(&mut app, " + PMI");
        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none());
        let after = crate::db::bill::list(&app.db, bill.category).unwrap();
        assert_eq!(after.len(), before);
        assert_eq!(
            after.iter().find(|b| b.id == bill.id).unwrap().label,
            format!("{} + PMI", bill.label)
        );
    }

    /// Every other row is a constant, which `e` already edits in place.
    #[test]
    fn capital_e_on_a_row_that_is_not_a_bill_is_refused() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        assert_eq!(app.planning.selected_target(), Some(Target::Target));

        press(&mut app, KeyCode::Char('E'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("bill"), "{}", app.status);
    }

    /// `d` on a constant would otherwise have to mean "delete a setting",
    /// which is what `p` does for the one setting that can be absent.
    #[test]
    fn d_on_a_row_that_is_not_a_bill_is_refused() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('d'));

        assert!(app.modal.is_none());
        assert!(app.status.contains("bill"), "{}", app.status);
    }

    /// `App::new` must survive a database the waterfall cannot run against --
    /// every other screen still works there.
    #[test]
    fn the_planning_screen_says_so_when_there_is_no_checking_account() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('5'));
        assert!(app.planning.rows().is_empty());
        assert!(
            app.planning.message().unwrap().contains("Checking band"),
            "{:?}",
            app.planning.message()
        );
    }

    /// Pinning freezes the excess the whole waterfall is divided from, so it
    /// takes the whole-dollar floor -- the same figure `compute` uses when
    /// nothing is pinned.
    #[test]
    fn p_pins_the_live_excess_at_its_whole_dollar_floor() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let live = app.planning.excess_actual();
        assert_ne!(live, live.floor_to_dollar(), "the fixture must have cents");

        press(&mut app, KeyCode::Char('p'));

        assert_eq!(
            setting::get(&app.db, key::PINNED_EXCESS).unwrap(),
            Some(live.floor_to_dollar())
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(today())
        );
        assert!(app.planning.is_pinned());
        assert!(app.planning.pin_line().unwrap().contains("pinned"));
    }

    /// Unpinning has to remove both keys -- a pin with a date and no amount
    /// would render a line about a plan that is not pinned.
    #[test]
    fn capital_p_unpins_and_clears_the_date_with_it() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('P'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&app.db, key::PINNED_AT).unwrap(), None);
        assert!(!app.planning.is_pinned());
        assert_eq!(app.planning.pin_line(), None);
    }

    /// The whole of the change: a second `p` re-pins rather than clearing.
    /// The press that follows a forgotten pin is the next payday's, and a `p`
    /// that answered it with "unpinned" would make the press that matters the
    /// second one every time.
    #[test]
    fn p_on_an_already_pinned_plan_re_pins_rather_than_clearing() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let first = setting::get(&app.db, key::PINNED_EXCESS).unwrap();
        assert!(first.is_some());

        press(&mut app, KeyCode::Char('p'));

        assert!(app.planning.is_pinned(), "the second p cleared the pin");
        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), first);
        assert_eq!(app.status, format!("re-pinned {}", first.unwrap()));
    }

    /// A re-pin takes the figure the screen is showing *now*, and moves the
    /// date with it -- so the drift falls back to the cents the whole-dollar
    /// floor drops, rather than going on reporting a gap against a figure
    /// that has just been replaced.
    #[test]
    fn a_re_pin_takes_the_current_excess_and_resets_the_drift() {
        let mut app = planning_app_with_a_row_after_today();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let first = setting::get(&app.db, key::PINNED_EXCESS)
            .unwrap()
            .expect("p did not pin");

        // Scrubbing past the Rent moves `Excess (Actual)` off the pin, which
        // is what the drift line reports. The scrub is an Overview key, and
        // it reaches Planning through the `App::adhoc` both screens read.
        press(&mut app, KeyCode::Char('1'));
        scrub_past_the_rent(&mut app);
        press(&mut app, KeyCode::Char('5'));
        assert!(app.planning.pin_line().unwrap().contains("moved"));

        press(&mut app, KeyCode::Char('p'));

        let second = setting::get(&app.db, key::PINNED_EXCESS)
            .unwrap()
            .expect("the re-pin cleared it");
        assert_ne!(second, first, "the re-pin kept the stale figure");
        assert_eq!(second, app.planning.excess_actual().floor_to_dollar());
        // Under a dollar, which is the floor's own remainder -- a fresh pin
        // never reads as zero drift, and it was a whole Rent out a moment ago.
        assert!(
            app.planning.excess_actual() - second < Cents::from_dollars(1),
            "{:?}",
            app.planning.pin_line()
        );
        assert_eq!(
            setting::get(&app.db, key::PINNED_AT).unwrap(),
            Some(app.today),
            "the re-pin left the old date behind"
        );
    }

    /// `P` on a plan nobody pinned says so rather than writing anything.
    #[test]
    fn capital_p_with_nothing_pinned_says_so() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('P'));

        assert_eq!(app.status, "nothing pinned");
        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
    }

    /// The pin is what `Excess (Used)` divides from, so pinning must be
    /// visible in the waterfall itself and not only in the footer.
    #[test]
    fn a_pinned_plan_uses_the_pinned_figure_rather_than_the_live_one() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        let pinned = planning_row(&app, "Excess (Used)").value.clone();

        // Move the live excess by lowering the target, and confirm the used
        // figure does not follow it.
        setting::set(&app.db, key::PLANNING_TARGET, Cents::from_dollars(1)).unwrap();
        app.reload().unwrap();

        assert_eq!(planning_row(&app, "Excess (Used)").value, pinned);
        assert!(
            app.planning
                .pin_line()
                .unwrap()
                .contains("excess has since moved"),
            "{:?}",
            app.planning.pin_line()
        );
    }

    /// Unpinning clears two keys and reads nothing off the view, and the
    /// footer offers `P unpin` on the strength of `is_pinned`, which survives
    /// `set_unavailable`. Refusing here would advertise a key that then does
    /// nothing, with the pin stuck until the plan computes again.
    #[test]
    fn a_pinned_plan_can_be_unpinned_after_it_stops_computing() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));
        assert!(app.planning.is_pinned());

        // A gate pointing at a goal that does not exist is a corrupt
        // database, which is exactly the error the screen reports rather than
        // computing a plan.
        setting::set(&app.db, crate::gate::Gate::Roth.key(), GoalId(999)).unwrap();
        app.reload().unwrap();
        assert!(app.planning.message().is_some());

        press(&mut app, KeyCode::Char('P'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert_eq!(setting::get(&app.db, key::PINNED_AT).unwrap(), None);
        assert_eq!(app.status, "unpinned");
    }

    /// `set_unavailable` leaves `excess_actual` holding whatever the last
    /// successful view left there, so `p` on a screen showing "no Everyday cash
    /// account" must not pin a number belonging to a plan the screen has just
    /// said it cannot compute.
    #[test]
    fn p_on_a_screen_with_no_live_view_is_refused() {
        let db = db::open_in_memory().unwrap();
        let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
        let mut app = App::new(db, today()).unwrap();

        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('p'));

        assert_eq!(setting::get(&app.db, key::PINNED_EXCESS).unwrap(), None);
        assert!(!app.planning.is_pinned());
        assert_eq!(app.status, "nothing to pin");
    }

    /// The destination block is the resolution, not three hardcoded names:
    /// nulling a line's key must move its money on the screen with no code
    /// change. This is the test the whole indirection exists for.
    ///
    /// "Future Housing" also labels the Split section's percentage row, and
    /// -- while the key is still set -- the destination block's own row for
    /// a line that resolved into a container. Both render on every load, so
    /// merely finding the label anywhere on screen, or even counting its
    /// occurrences, proves nothing: the count is identical whether the line
    /// landed in Brokerage or fell out as a withdrawal. What changes is
    /// which row comes immediately before it: a withdrawal is always the
    /// row directly under its own `Withdrawal` heading -- one line's
    /// `(Line, Cents)` pair per pair of rows, from `transfer::Row::Withdrawal`.
    #[test]
    fn nulling_a_lines_destination_key_moves_it_out_of_its_group_on_screen() {
        let mut app = planning_app();
        let before = planning_row(&app, "Brokerage").value.clone();
        let future_housing_is_a_withdrawal = |app: &App| {
            app.planning
                .rows()
                .windows(2)
                .any(|w| w[0].label.trim() == "Withdrawal" && w[1].label.trim() == "Future Housing")
        };
        assert!(
            !future_housing_is_a_withdrawal(&app),
            "Future Housing should still be inside Brokerage, not a withdrawal, before the key is cleared"
        );

        let Destination::Goal(key) = Line::FutureHousing.destination() else {
            panic!("Future Housing is goal-backed");
        };
        setting::clear(&app.db, key).unwrap();
        app.reload().unwrap();

        assert_ne!(planning_row(&app, "Brokerage").value, before);
        assert!(
            future_housing_is_a_withdrawal(&app),
            "the line did not appear as a withdrawal"
        );
    }

    /// `t` writes every row in one go and leaves the ledger holding both
    /// legs of each transfer.
    #[test]
    fn t_on_planning_writes_the_transfers() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        // `planning_app` already seeds one paycheck row, so the assertion
        // below is against the count `t` itself adds, not against zero.
        let before = crate::db::txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        assert!(crate::db::txn::count(&app.db).unwrap() > before);
    }

    /// `planning_app` leaves Lego and Dropbox unclaimed in Brokerage. One
    /// unclaimed goal in Rainy Day as well is all it takes for the plug to have
    /// nowhere single to land -- the owner's own database, in miniature.
    fn ambiguous_app() -> App {
        let mut app = planning_app();
        let savings = account::by_code(&app.db, "SAV", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Sabbatical".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 9,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();
        app
    }

    /// The complaint this answers: the screen says the plan is unresolved in
    /// a cell too narrow to name the goal that caused it.
    #[test]
    fn enter_on_an_unresolved_plan_opens_the_details() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Enter);

        let Some(Modal::Details(title, lines)) = &app.modal else {
            panic!("no details panel opened: {}", app.status);
        };
        assert_eq!(*title, "Transfers unresolved");
        let text = lines.join("\n");
        assert!(text.contains("Rainy Day"), "{text}");
        assert!(text.contains("Brokerage"), "{text}");
        assert!(text.contains("Sabbatical"), "{text}");
    }

    /// The row and the panel are two lengths of one failure, and the panel is
    /// the one with room for the part that says what to do about it.
    #[test]
    fn the_panel_names_the_goal_the_unresolved_row_has_no_room_for() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));
        let row = app
            .planning
            .rows()
            .iter()
            .find(|r| r.label.trim() == "unresolved")
            .expect("no unresolved row");
        assert!(!row.value.contains("Sabbatical"), "{}", row.value);

        press(&mut app, KeyCode::Enter);
        let Some(Modal::Details(_, lines)) = &app.modal else {
            panic!("no details panel opened");
        };
        assert!(lines.join("\n").contains("Sabbatical"));
    }

    #[test]
    fn esc_closes_the_details() {
        let mut app = ambiguous_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        assert!(app.modal.is_none());
    }

    /// A key that opens an empty panel is worse than one that says why it
    /// did nothing.
    #[test]
    fn enter_on_a_resolved_plan_opens_nothing_and_says_so() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));

        press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "a panel opened over a resolved plan");
        assert!(app.status.contains("nothing to explain"), "{}", app.status);
    }

    /// End to end, through the keyboard: closing the goal a line names must
    /// not turn the next `e`-`Enter` into a silent clearing.
    #[test]
    fn e_then_enter_on_a_line_naming_a_closed_goal_changes_nothing() {
        let mut app = planning_app();
        let key = destination_key(Line::Bills);
        let closed = setting::get(&app.db, key).unwrap().unwrap();
        goal::close(&app.db, closed).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, key).unwrap(),
            Some(closed),
            "a bare Enter cleared a destination nobody questioned"
        );
    }

    /// The six scroll keys are documented nowhere because they work on every
    /// list in the app. A modal whose height never reaches its cursor keeps
    /// `page_height: 1`, so `PageDown` degenerates to `Down` -- an
    /// undocumented key quietly not working, which is the one failure mode
    /// that promise cannot survive.
    #[test]
    fn page_down_moves_a_screenful_in_the_destination_list() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));
        // The viewport height is a render-time measurement, so the list has
        // to be drawn once before a page means anything.
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 40)).unwrap();
        terminal.draw(|frame| app.render(frame)).unwrap();

        press(&mut app, KeyCode::PageDown);

        let chooser = destination(&app);
        assert!(
            chooser.selected_index() > 1,
            "PageDown moved {} row(s) -- the viewport height never reached the cursor",
            chooser.selected_index()
        );
    }

    /// Walk the Planning cursor down to one line's destination row.
    fn cursor_to(app: &mut App, line: Line) {
        for _ in 0..app.planning.rows().len() {
            if app.planning.selected_editable()
                == Some(planning_screen::Editable::Destination(line))
            {
                return;
            }
            press(app, KeyCode::Down);
        }
        panic!("no destination row for {line:?}");
    }

    fn goal_named(app: &App, name: &str) -> GoalId {
        goal::all_with_balances(&app.db)
            .unwrap()
            .into_iter()
            .find(|g| g.goal.name == name)
            .unwrap_or_else(|| panic!("no goal named {name:?}"))
            .goal
            .id
    }

    /// The whole feature, end to end: a line whose key an older import never
    /// wrote, pointed at the goal it names without leaving the app.
    #[test]
    fn e_then_enter_points_an_unset_line_at_the_goal_its_name_suggests() {
        let mut app = planning_app();
        setting::clear(&app.db, destination_key(Line::MomAndDad)).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::MomAndDad);

        press(&mut app, KeyCode::Char('e'));
        assert!(
            matches!(app.modal, Some(Modal::Destination(_))),
            "no destination list opened: {}",
            app.status
        );

        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, destination_key(Line::MomAndDad)).unwrap(),
            Some(goal_named(&app, "Mom & Dad"))
        );
        assert!(app.modal.is_none(), "the list stayed open");
        assert!(app.status.contains("Mom & Dad"), "{}", app.status);
    }

    /// The reverse, and the reason the withdrawal is a row rather than an
    /// `Esc`: unset is a destination the owner may want to choose.
    #[test]
    fn choosing_the_withdrawal_clears_the_lines_key() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        // The list opens on the goal this line already names, lifted to the
        // top, so the withdrawal is the row directly under it -- one Down,
        // whatever that goal's own place in the list would have been.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            setting::get(&app.db, destination_key(Line::Bills)).unwrap(),
            None
        );
    }

    /// The screen behind the list is rebuilt from the setting that was just
    /// written -- otherwise the block still shows the state it was opened to
    /// change.
    #[test]
    fn the_block_shows_the_new_destination_as_soon_as_the_list_closes() {
        let mut app = planning_app();
        setting::clear(&app.db, destination_key(Line::MomAndDad)).unwrap();
        app.reload().unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::MomAndDad);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Enter);

        let row = app
            .planning
            .rows()
            .iter()
            .find(|r| r.editable == Some(planning_screen::Editable::Destination(Line::MomAndDad)))
            .expect("no Mom & Dad destination row");
        assert_eq!(row.value, "Mom & Dad");
        assert_eq!(row.extra, "Brokerage");
    }

    #[test]
    fn esc_leaves_the_destination_alone() {
        let mut app = planning_app();
        let before = setting::get(&app.db, destination_key(Line::Bills)).unwrap();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Esc);

        assert!(app.modal.is_none());
        assert_eq!(
            setting::get(&app.db, destination_key(Line::Bills)).unwrap(),
            before
        );
    }

    /// `/` opens a box that takes typed characters, so `?` types into it
    /// rather than opening the panel -- the rule every other search box
    /// follows.
    #[test]
    fn a_question_mark_types_into_the_destination_search_rather_than_opening_the_panel() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));

        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('?'));

        assert!(app.help.is_none(), "the panel opened over the search box");
        let chooser = destination(&app);
        let title: String = chooser
            .title()
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(title.contains("/?"), "{title}");

        // F1 is the way in while a question mark is a character.
        press(&mut app, KeyCode::F(1));
        assert!(open_on(&app, Topic::DestinationSearch));
    }

    /// `TransferConfirm` has one field and it is a date, which holds no literal
    /// `?` -- the same reasoning that excludes the worksheet's date focus -- so
    /// a plain `?` reaches the panel rather than typing into the field.
    #[test]
    fn a_plain_question_mark_opens_the_panel_over_the_transfer_confirmation() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        press(&mut app, KeyCode::Char('t'));
        assert!(
            matches!(app.modal, Some(Modal::PlanTransfers(_))),
            "no transfer confirmation opened"
        );

        press(&mut app, KeyCode::Char('?'));
        assert!(open_on(&app, Topic::PlanTransfers));
        assert!(
            matches!(app.modal, Some(Modal::PlanTransfers(_))),
            "the confirmation is still queued behind the panel"
        );
    }

    /// A second run on the same date is a real case -- a corrected date --
    /// so it warns rather than blocks.
    #[test]
    fn a_second_t_on_the_same_date_warns_without_blocking() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        // `planning_app` already seeds one paycheck row, so the payday's own
        // count is `after_first - before`, not `after_first` outright.
        let before = crate::db::txn::count(&app.db).unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        let after_first = crate::db::txn::count(&app.db).unwrap();
        let payday_rows = after_first - before;
        // The commit opened the receiving containers' worksheets in turn;
        // dismiss both before the second `t`, which is a Planning-screen key.
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none(), "a worksheet is still queued");

        press(&mut app, KeyCode::Char('t'));
        assert!(app.status.contains("already"), "{}", app.status);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Esc);

        assert_eq!(
            crate::db::txn::count(&app.db).unwrap(),
            after_first + payday_rows
        );
    }

    /// A key pointing at a goal that no longer exists is a corrupt database,
    /// not an empty payday. `t` must refuse it before the modal opens, not
    /// let the owner confirm a plan `transfer::plan` could not resolve.
    #[test]
    fn t_on_an_unresolved_plan_refuses_and_opens_no_modal() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        let Destination::Goal(key) = Line::MomAndDad.destination() else {
            panic!("Mom & Dad is goal-backed");
        };
        setting::set(&app.db, key, GoalId(9_999)).unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert!(
            app.modal.is_none(),
            "a modal opened over an unresolved plan"
        );
        assert!(
            app.status.contains("planning.goal.mom_and_dad_id"),
            "{}",
            app.status
        );
    }

    /// Committing the transfers opens the receiving containers' worksheets in
    /// turn, each prefilled with the lines that landed there and the plug
    /// spread over the goals no line claims.
    #[test]
    fn committing_the_transfers_opens_each_containers_worksheet_in_turn() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let first = match &app.modal {
            Some(Modal::Worksheet(sheet)) => sheet.container(),
            _ => panic!("no worksheet opened"),
        };
        // Esc closes one worksheet and the next opens behind it.
        press(&mut app, KeyCode::Esc);
        let second = match &app.modal {
            Some(Modal::Worksheet(sheet)) => sheet.container(),
            _ => panic!("the second worksheet did not open"),
        };
        assert_ne!(first, second);

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none(), "a third worksheet opened");
    }

    /// The allocation is the transfer seen from the container's side, so both
    /// carry the one date -- and it is the date the owner confirmed, not the
    /// default that date was stepped off: a worksheet dated today would credit
    /// the goals days before the cash the transfer moves reaches them.
    #[test]
    fn each_worksheet_opens_on_the_date_the_transfers_were_written_for() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        press(&mut app, KeyCode::Char('t'));
        let confirm = plan_transfers(&app);
        let default = confirm.commit().unwrap();

        // Stepped off the two-business-day default, so a sheet that reached
        // for either that default or today fails this.
        press(&mut app, KeyCode::Right);
        let confirm = plan_transfers(&app);
        let date = confirm.commit().unwrap();
        assert_eq!(date, default + chrono::TimeDelta::days(1));
        assert_ne!(date, app.today);

        press(&mut app, KeyCode::Enter);

        let sheet = worksheet(&app);
        assert_eq!(sheet.date_text(), date.to_string());
        press(&mut app, KeyCode::Esc);
        let sheet = worksheet(&app);
        assert_eq!(sheet.date_text(), date.to_string());
    }

    /// The Rainy Day worksheet's pot is the transfer, and its lines are the
    /// claimed goals at their own amounts plus the plug spread over the rest
    /// -- so the sheet opens reconciled and the owner reviews rather than
    /// retypes.
    #[test]
    fn the_first_worksheet_opens_with_the_transfer_as_its_pot() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        // The plan behind this payday, read fresh: the same figures `t`
        // itself just used to build the confirm modal and the prefill.
        let plan = computed_plan(&app, app.adhoc);
        let Destination::Goal(bills_key) = Line::Bills.destination() else {
            panic!("Bills is goal-backed");
        };
        let bill_payments = setting::get(&app.db, bills_key).unwrap().unwrap();
        let Destination::Goal(roth_key) = Line::Roth.destination() else {
            panic!("Roth is goal-backed");
        };
        let roth = setting::get(&app.db, roth_key).unwrap().unwrap();

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let sheet = worksheet(&app);
        assert_eq!(
            sheet.remaining(),
            Cents::ZERO,
            "the sheet did not open reconciled"
        );
        assert!(sheet.amount() > Cents::ZERO);

        // The central claim: each claimed goal carries its own line's
        // amount, not a share of the pot dumped somewhere convenient.
        let line = |id: GoalId| {
            sheet
                .lines()
                .into_iter()
                .find(|l| l.goal_id == id)
                .unwrap_or_else(|| panic!("no worksheet line for goal {id}"))
        };
        assert_eq!(line(bill_payments).amount, plan.lines.bills);
        assert_eq!(
            line(roth).amount,
            Cents::ZERO,
            "the Roth gate is already met, so plan emits no row for it"
        );
    }

    /// Nothing stops two lines naming one goal -- `open_destination` offers
    /// every open goal, claimed or not -- and `transfer::plan` merges them
    /// into one transfer. The prefill has to merge them the same way, or the
    /// sheet opens short by the second line and the difference is
    /// indistinguishable from the remainder `calc::fit` leaves on purpose.
    ///
    /// Housing is funded before it is unpointed: a goal no line claims and
    /// which is still short joins the plug, and a plug spanning Rainy Day and
    /// Brokerage is a refusal rather than the case under test.
    #[test]
    fn two_lines_naming_one_goal_prefill_it_with_their_sum() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let savings = account::by_code(&app.db, "SAV", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        let goal_id = |name: &str| {
            goal::list_with_balances(&app.db, savings)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no {name:?} goal in Rainy Day"))
                .goal
                .id
        };
        let bill_payments = goal_id("Bill Payments");
        goal::insert_allocation(
            &app.db,
            goal_id("Housing"),
            day(2026, 8, 1),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
        let Destination::Goal(housing_key) = Line::CurrentHousing.destination() else {
            panic!("Current Housing is goal-backed");
        };
        setting::set(&app.db, housing_key, bill_payments).unwrap();
        app.reload().unwrap();
        let plan = computed_plan(&app, app.adhoc);

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);

        let sheet = worksheet(&app);
        let line = sheet
            .lines()
            .into_iter()
            .find(|l| l.goal_id == bill_payments)
            .expect("no worksheet line for Bill Payments");
        assert!(plan.lines.current_housing > Cents::ZERO, "nothing to lose");
        assert_eq!(line.amount, plan.lines.bills + plan.lines.current_housing);
        assert_eq!(
            sheet.remaining(),
            Cents::ZERO,
            "the sheet did not open reconciled"
        );
    }

    /// The rows a payday of this fixture writes, put on the ledger at
    /// `date` -- a first run, so a later `t` has something to clash with.
    fn payday_landed_on(app: &App, date: NaiveDate) {
        let from = transfer::source(&app.db).unwrap();
        let plan = computed_plan(app, app.adhoc);
        let rows = transfer::plan(&app.db, &plan.lines).unwrap();
        transfer::execute(&app.db, from, date, &rows).unwrap();
    }

    /// The form's date is editable, so the day a first run landed on is not
    /// the day the second run opens on -- which is the whole case the warning
    /// exists for. It scans business days either side and names what it
    /// found, because those are days the owner cannot see on the form.
    ///
    /// The fixture's today is a Saturday, so the default is Tuesday the 18th
    /// and Monday the 17th is one business day behind it.
    #[test]
    fn t_warns_when_a_neighbouring_business_day_already_carries_the_payday() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 17));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert_eq!(app.status, "2026-08-17 already carries matching rows");
    }

    /// Two clashes are both named: the owner is picking a date, and being
    /// told about one of the two days to avoid is worse than being told the
    /// count.
    #[test]
    fn t_names_every_neighbouring_day_that_clashes() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 17));
        payday_landed_on(&app, day(2026, 8, 19));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert_eq!(
            app.status,
            "2026-08-17 and 2026-08-19 already carry matching rows"
        );
    }

    /// The window has an edge, and past it the warning stays quiet: Friday
    /// the 21st is three business days past the 18th.
    #[test]
    fn t_is_quiet_about_a_payday_beyond_the_window() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        payday_landed_on(&app, day(2026, 8, 21));
        app.reload().unwrap();

        press(&mut app, KeyCode::Char('t'));

        assert!(app.status.is_empty(), "{}", app.status);
    }

    /// `planning_app`'s spare goals live in Brokerage, so the Rainy Day sheet
    /// above never touches `worksheet_prefills`' `Destination::Spread`
    /// branch at all -- the second sheet, Brokerage, is the one worksheet
    /// that does.
    ///
    /// Dated here rather than in the fixture: an undated goal asks for
    /// nothing, and a sheet where every spare goal asks nothing exercises the
    /// division not at all. The sibling test below is the undated case.
    #[test]
    fn the_second_worksheet_prefills_the_plugs_goals_with_what_they_ask() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let plan = computed_plan(&app, app.adhoc);
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        let goal_id = |name: &str| {
            goal::list_with_balances(&app.db, brokerage)
                .unwrap()
                .into_iter()
                .find(|g| g.goal.name == name)
                .unwrap_or_else(|| panic!("no {name:?} goal in Brokerage"))
                .goal
                .id
        };
        let down_payment = goal_id("Home Down Payment");
        let lego = goal_id("Lego");
        let dropbox = goal_id("Dropbox");
        // One paycheck away, so each asks for the whole of what it lacks:
        // $1,000 apiece against a plug of several thousand, which fits.
        for id in [lego, dropbox] {
            goal::update(
                &app.db,
                id,
                &goal::GoalEdit {
                    name: goal::get(&app.db, id).unwrap().unwrap().name,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: Some(app.today + chrono::Duration::days(7)),
                    interest_eligible: true,
                    taxed: false,
                },
            )
            .unwrap();
        }

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        let sheet = worksheet(&app);
        assert_eq!(
            sheet.amount(),
            plan.lines.future_housing + plan.lines.mom_and_dad + plan.lines.goals
        );

        let line = |id: GoalId| {
            sheet
                .lines()
                .into_iter()
                .find(|l| l.goal_id == id)
                .unwrap_or_else(|| panic!("no worksheet line for goal {id}"))
        };
        assert_eq!(line(down_payment).amount, plan.lines.future_housing);

        // Each asked for the $1,000 it lacks and got it, whole.
        assert_eq!(line(lego).amount, Cents::from_dollars(1_000));
        assert_eq!(line(dropbox).amount, Cents::from_dollars(1_000));

        // And the rest of the plug is left where the owner can place it,
        // rather than shared out over goals that did not ask for it.
        assert_eq!(
            sheet.remaining(),
            plan.lines.goals - Cents::from_dollars(2_000),
        );
    }

    /// The plug's gap is the one figure on the Planning screen that comes
    /// from outside the waterfall: `transfer::spread_asks`, over the same
    /// goals `t`'s own prefill divides that line between. Asserted through
    /// `App`, because nothing in `tui::planning` reads a database -- a `View`
    /// left carrying zero would draw a silent footer on every real payday and
    /// every test in that module would still pass.
    #[test]
    fn the_planning_screens_plug_is_measured_against_its_own_goals_asks() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        // In the container the plug already spreads over, dated one pay
        // period out -- so it asks for the whole of what it lacks, which is
        // far past anything this payday moves.
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Roof".to_string(),
                container_account_id: brokerage,
                base_cents: Cents::from_dollars(500_000),
                goal_date: Some(app.today + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        app.reload().unwrap();

        let start = app
            .planning
            .rows()
            .iter()
            .position(|r| r.label == "Transfers")
            .expect("no Transfers heading");
        let footer = app.planning.rows()[start..]
            .iter()
            .take_while(|r| !(r.label.is_empty() && r.value.is_empty()))
            .find(|r| r.label.trim() == "Unmet Asks")
            .expect("no Unmet Asks footer among the transfers");

        let plan = computed_plan(&app, app.adhoc);
        let gap = plan.lines.goals - Cents::from_dollars(500_000);
        assert_eq!(footer.extra, format!("\u{394} {}", gap.to_whole_dollars()));
    }

    /// The payday `Unmet Asks` exists for is the one where every line is
    /// zero, and that is the payday `transfer::plan` refuses outright -- so
    /// the asks the footer measures the plug against cannot be read through
    /// that call. Asserted through `App` for the reason the test above it is:
    /// a `View` built by hand carries whatever total it is given, and every
    /// test in `tui::planning` would still pass over a screen that had gone
    /// silent on the one payday the footer exists for.
    #[test]
    fn a_payday_with_nothing_to_transfer_still_reports_what_its_goals_asked() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let brokerage = account::by_code(&app.db, "BKR", Kind::Cash)
            .unwrap()
            .unwrap()
            .id;
        goal::insert(
            &app.db,
            &goal::NewGoal {
                name: "Roof".to_string(),
                container_account_id: brokerage,
                base_cents: Cents::from_dollars(500_000),
                goal_date: Some(app.today + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();
        // An excess of nothing puts every line at zero -- what a payday whose
        // fixed bills took the whole of it produces, and what `plan` refuses.
        setting::set(&app.db, key::PINNED_EXCESS, Cents::ZERO).unwrap();
        app.reload().unwrap();

        let rows = app.planning.rows().to_vec();
        let start = rows
            .iter()
            .position(|r| r.label == "Transfers")
            .expect("no Transfers heading");
        let block: Vec<_> = rows[start..]
            .iter()
            .take_while(|r| !(r.label.is_empty() && r.value.is_empty()))
            .collect();

        assert!(
            block
                .iter()
                .any(|r| r.label.trim() == transfer::NOTHING_TO_TRANSFER),
            "the block is not the one this payday produces: {block:?}"
        );
        let footer = block
            .iter()
            .find(|r| r.label.trim() == "Unmet Asks")
            .expect("no Unmet Asks footer on a payday with nothing to transfer");
        let gap = Cents::ZERO - Cents::from_dollars(500_000);
        assert_eq!(footer.extra, format!("\u{394} {}", gap.to_whole_dollars()));
    }

    /// The plug is priced against the **target**, so a taxed goal funded to
    /// its base is still short by the tax and still asks for it. Asserted
    /// through `App` rather than on `transfer::spread_asks` directly, because
    /// what this pins is that the two dates the ask divides by are the ones
    /// the application is running on.
    #[test]
    fn a_taxed_goals_plug_ask_is_measured_against_its_taxed_target() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = goal::insert(
            &db,
            &goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                // One pay period (14 days) past `today`, so the ask is the
                // whole of what is lacking rather than a fraction of it.
                goal_date: Some(today() + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, id, today(), Cents::from_dollars(1_000), None, None).unwrap();

        let app = App::new(db, today()).unwrap();
        let asks = app.spread_asks().unwrap();
        let (_, ask) = asks
            .into_iter()
            .find(|(g, _)| g.id == id)
            .unwrap_or_else(|| panic!("no plug entry for the taxed goal"));

        // 1,000 taxed at 6.25% is 1,062.50, carried up to 1,065 by the
        // lambda's $5 increment. Funded to the base, the goal still lacks
        // the $65 of tax.
        assert_eq!(ask, Cents::from_dollars(65));
    }

    /// An undated goal has no runway to divide, so it asks for nothing and
    /// the whole plug is left unallocated. That is the intended answer, not a
    /// gap: money nobody has dated a use for is money to place by hand.
    #[test]
    fn a_plug_whose_goals_ask_for_nothing_is_left_unallocated() {
        let mut app = planning_app();
        app.screen = Screen::Planning;
        let plan = computed_plan(&app, app.today);

        press(&mut app, KeyCode::Char('t'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Esc);

        let sheet = worksheet(&app);
        assert_eq!(sheet.remaining(), plan.lines.goals);
    }

    /// `worksheet_prefills` calls `spread_container` unconditionally, even
    /// when this payday's own Goals plug is zero and `transfer::plan` itself
    /// never needed a spread destination. A database whose unclaimed goals
    /// span two containers must therefore refuse before anything is
    /// written, not after: `already_written` is never consulted on a
    /// re-confirm, so a refusal stranded after `transfer::execute` would let
    /// a second `Enter` duplicate the whole payday.
    #[test]
    fn a_zero_plug_with_unclaimed_goals_in_two_containers_refuses_before_writing() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();
        // A round $10,000.00, so Future Housing/Retirement/Investment's
        // 40/30/30 split lands on whole dollars with nothing left for Goals
        // to absorb.
        write(&db, checking, day(2026, 8, 1), 1_000_000, "Paycheck");

        let new_goal = |container: AccountId, name: &str, sort: i64| -> GoalId {
            goal::insert(
                &db,
                &goal::NewGoal {
                    name: name.to_string(),
                    container_account_id: container,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort,
                    taxed: false,
                },
            )
            .unwrap()
        };
        let down_payment = new_goal(brokerage, "Home Down Payment", 0);
        // Two unclaimed goals, one per container, so `unclaimed_goals` spans
        // both -- the exact state `spread_container` refuses to divide the
        // plug over.
        new_goal(savings, "Stray Rainy Day", 0);
        new_goal(brokerage, "Stray Brokerage", 1);

        let Destination::Goal(fh_key) = Line::FutureHousing.destination() else {
            panic!("Future Housing is goal-backed");
        };
        setting::set(&db, fh_key, down_payment).unwrap();
        // No bills, no Mom & Dad, and Future Housing + Retirement +
        // Investment sum to exactly 100% of a whole-dollar remainder: every
        // other line is zero and the three that aren't leave no residue for
        // Goals to absorb, so the plug lands on exactly `Cents::ZERO`
        // rather than a rounding leftover.
        setting::set(&db, key::PLANNING_TARGET, Cents::ZERO).unwrap();
        setting::set(&db, key::PLANNING_BUFFER, Cents::ZERO).unwrap();
        setting::set(&db, key::BILL_PAYMENT_CAP, Cents::ZERO).unwrap();
        setting::set(&db, key::MOM_AND_DAD_ANNUAL, Cents::ZERO).unwrap();
        setting::set(&db, key::SPLIT_FUTURE_HOUSING_PCT, Percent(40)).unwrap();
        setting::set(&db, key::SPLIT_RETIREMENT_PCT, Percent(30)).unwrap();
        setting::set(&db, key::SPLIT_INVESTMENT_PCT, Percent(30)).unwrap();

        let mut app = App::new(db, today()).unwrap();
        app.screen = Screen::Planning;
        let plan = computed_plan(&app, app.adhoc);
        assert_eq!(
            plan.lines.goals,
            Cents::ZERO,
            "the fixture must zero the plug for this test to mean anything"
        );

        let before = crate::db::txn::count(&app.db).unwrap();
        press(&mut app, KeyCode::Char('t'));
        assert!(
            app.modal.is_some(),
            "the confirm modal did not open: {}",
            app.status
        );

        press(&mut app, KeyCode::Enter);

        assert_eq!(
            crate::db::txn::count(&app.db).unwrap(),
            before,
            "the payday wrote rows before the container-spanning plug was refused"
        );
        assert!(
            app.status.contains("Rainy Day") && app.status.contains("Brokerage"),
            "{}",
            app.status
        );
    }

    #[test]
    fn esc_with_a_kept_filter_clears_it_rather_than_closing_the_destination_list() {
        let mut app = planning_app();
        press(&mut app, KeyCode::Char('5'));
        cursor_to(&mut app, Line::Bills);
        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('/'));
        type_str(&mut app, "zzz");
        press(&mut app, KeyCode::Enter);

        press(&mut app, KeyCode::Esc);
        let chooser = destination(&app);
        assert_eq!(chooser.search(), "");

        press(&mut app, KeyCode::Esc);
        assert!(app.modal.is_none());
    }
}
