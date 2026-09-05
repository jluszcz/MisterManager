//! What is layered over a screen, and everything that is true of it whichever
//! screen opened it: which modals carry fields, which help topic each is
//! showing, how each one draws, and -- for a confirmation dialog -- what it
//! asks and what `y` writes.
//!
//! Those were four matches over one enum, scattered the length of `app`, and
//! adding a modal meant finding all four. `modal_key` is the one that stayed
//! there: `App` owns the `Option<Modal>` and every handler a key reaches, so
//! each of its arms is a call into one, and it belongs beside the handlers
//! rather than beside the enum.

use super::accounts::{self as accounts_screen, AccountForm};
use super::autocomplete::Autocomplete;
use super::cursor::Scroll;
use super::destination;
use super::form::{self, FormFields, ValueForm};
use super::fund::{self as fund_screen, FundForm};
use super::goal_form::{self, AllocationForm, CloseForm, GoalForm, GoalTransferForm};
use super::help::Topic;
use super::history::{self, History, Mode as HistoryMode};
use super::ledger_form::{self, TransferForm, TxnForm};
use super::picker::{self, Picker};
use super::planning::{self, BillForm, Target, TransferConfirm};
use super::recurring_goal::{self as recurring_goal_screen, RecurringGoalForm};
use super::recurring_txn::{self as recurring_txn_screen, RecurringTxnForm};
use super::search::Search;
use super::widget;
use super::worksheet::{self, Worksheet};
use crate::db::bill;
use crate::db::fund;
use crate::db::goal;
use crate::db::recurring_goal;
use crate::db::recurring_txn;
use crate::db::txn;
use crate::db::{
    AccountId, AllocationId, BatchId, BillId, Db, FundId, RecurringGoalId, RecurringTxnId, TxnId,
};
use anyhow::Result;
use ratatui::Frame;
use ratatui::text::Line as TextLine;

/// What is layered over the current screen, if anything.
pub(super) enum Modal {
    Txn(TxnForm),
    Transfer(TransferForm),
    Allocation(AllocationForm),
    Goal(GoalForm),
    CloseOut(CloseForm),
    /// `t` on a Savings row: part of that goal's value into another goal of
    /// the same container. Named apart from `Transfer`, which moves cash
    /// between accounts.
    GoalTransfer(GoalTransferForm),
    Worksheet(Worksheet),
    Picker(Picker),
    /// `d` and `U`: the last chance to back out of a write. `label` is the row
    /// as the screen it was pressed on describes it; `action` is everything
    /// else about the dialog.
    Confirm {
        action: Confirm,
        label: String,
    },
    /// One prefilled field, and the [`ValueTarget`] saying what it is being
    /// collected for.
    Value(ValueTarget, ValueForm),
    Bill(BillForm),
    RecurringTxn(RecurringTxnForm),
    /// `e` on a Planning destination row: the goals this line could be
    /// pointed at, and the withdrawal among them.
    Destination(destination::Chooser),
    /// `Enter` on Planning: why the transfers are unresolved, at the length
    /// a panel can hold rather than the length a table cell can.
    Details(&'static str, Vec<String>),
    RecurringGoalEntry(RecurringGoalForm),
    /// `t` on the Planning screen: the resolved rows and an editable date,
    /// confirmed before anything is written.
    PlanTransfers(TransferConfirm),
    Fund(FundForm),
    /// `e` on an account row: its name, band, position and interest policy.
    /// Everything the owner may say about an account, and nothing the
    /// workbook says.
    Account(AccountForm),
    /// `Enter` on a Savings row: one goal's allocation rows, and the two
    /// writes that correct one. Its own three modes live inside [`History`]
    /// rather than as a modal over a modal.
    History(History),
}

impl Modal {
    /// The field-driven forms. A `Confirm` has no fields, and `Worksheet` and
    /// `Picker` are list editing rather than fields, so those are `None` and
    /// never reach the shared `form_key` handler.
    pub(super) fn fields_mut(&mut self) -> Option<&mut dyn FormFields> {
        match self {
            Modal::Txn(form) => Some(form),
            Modal::Transfer(form) => Some(form),
            Modal::Allocation(form) => Some(form),
            Modal::Goal(form) => Some(form),
            Modal::CloseOut(form) => Some(form),
            Modal::GoalTransfer(form) => Some(form),
            Modal::Worksheet(_) => None,
            Modal::Picker(_) => None,
            Modal::Confirm { .. } => None,
            Modal::Value(_, form) => Some(form),
            Modal::Bill(form) => Some(form),
            Modal::RecurringTxn(form) => Some(form),
            Modal::Destination(_) => None,
            Modal::Details(..) => None,
            Modal::RecurringGoalEntry(form) => Some(form),
            Modal::PlanTransfers(_) => None,
            Modal::Fund(form) => Some(form),
            Modal::Account(form) => Some(form),
            // Only while the history is editing: in the other two modes it is
            // a list and a question, neither of which has a field.
            Modal::History(history) => match history.mode_mut() {
                HistoryMode::Editing(form) => Some(form),
                HistoryMode::List | HistoryMode::Confirming { .. } => None,
            },
        }
    }

    /// The keys that are live while this modal is up, which is every key: a
    /// modal wins over the screen it is drawn on.
    ///
    /// The `is_searching` arms come first because a search box takes the keys
    /// the modal's own operators would otherwise answer.
    pub(super) fn topic(&self) -> Topic {
        match self {
            // A pending slash is waiting for the next key to decide whether
            // it is a fraction or a filter, so that key is data either way.
            Modal::Worksheet(sheet) if sheet.is_searching() || sheet.is_pending_slash() => {
                Topic::WorksheetSearch
            }
            Modal::Worksheet(_) => Topic::Worksheet,
            Modal::Picker(_) => Topic::Picker,
            Modal::Destination(chooser) if chooser.is_searching() => Topic::DestinationSearch,
            Modal::Destination(_) => Topic::Destination,
            Modal::Details(..) => Topic::Details,
            Modal::Confirm { .. } => Topic::Confirm,
            Modal::Txn(_) | Modal::Transfer(_) | Modal::RecurringTxn(_) => Topic::SuggestForm,
            Modal::Allocation(_)
            | Modal::Goal(_)
            | Modal::CloseOut(_)
            | Modal::GoalTransfer(_)
            | Modal::Value(..)
            | Modal::Bill(_)
            | Modal::RecurringGoalEntry(_) => Topic::Form,
            // Under match guards, the construction `Modal::Worksheet` above
            // already uses for its search box: the footer follows the mode
            // without any screen asking it to.
            Modal::History(h) if matches!(h.mode(), HistoryMode::Editing(_)) => Topic::Form,
            Modal::History(h) if matches!(h.mode(), HistoryMode::Confirming { .. }) => {
                Topic::Confirm
            }
            Modal::History(_) => Topic::History,
            Modal::PlanTransfers(_) => Topic::PlanTransfers,
            Modal::Fund(_) => Topic::Form,
            Modal::Account(_) => Topic::Form,
        }
    }
}

/// What a [`Modal::Value`] is collecting: one prefilled field, and the thing
/// on the far side of it.
///
/// Four screens edit a single figure through this one modal, and the only
/// thing that differs between them is where `Enter` writes. A variant per
/// screen carrying an identical `ValueForm` would spell that difference out
/// four times over -- in `fields_mut`, in `topic`, in `render` and in
/// `modal_key`, three of which have nothing to say about it. Carried here,
/// only the handler asks.
///
/// One variant per thing that can be edited, for the reason [`Confirm`] is
/// one per thing that can be confirmed: a fifth figure cannot be added
/// without saying what commits it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ValueTarget {
    /// `e` on the Planning screen, parsed by the [`Target`] it was opened
    /// for.
    Planning(Target),
    /// `r` on a ledger narrowed to one account: the balance a statement says
    /// that account holds. Session state on the `Ledger`, so committing it
    /// writes nothing and reloads nothing.
    Reconcile(AccountId),
    /// `e` on a fund row: the value that fund holds.
    Fund(FundId),
    /// The Funds screen has an age row and no birth date on record. `Esc`
    /// dismisses it -- the screen still draws, with the age row's target as
    /// `—`.
    BirthDate,
}

/// What a [`Modal::Confirm`] is asking about: the row `y` writes to, and
/// which write that is.
///
/// One variant per thing that can be confirmed rather than a closure, so the
/// write stays an exhaustive match -- a seventh dialog cannot be added
/// without saying what `y` does, what the border asks, and how a cancel
/// reads.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Confirm {
    /// Writes commit to SQLite on `Enter`, so this is the only chance to back
    /// out.
    DeleteTxn(TxnId),
    /// The batch of allocations `U` would undo.
    UndoBatch(BatchId),
    /// Deleting a bill silently changes two waterfall lines and every
    /// transfer instruction below them.
    DeleteBill(BillId),
    /// The rows a recurring transaction owns are released rather than
    /// deleted, so no balance moves.
    DeleteRecurringTxn(RecurringTxnId),
    /// `recurring_goal::delete` itself refuses while any goal still
    /// references the entry, open or closed.
    DeleteRecurringGoal(RecurringGoalId),
    /// Nothing here holds money, but the row's share of the split disappears
    /// with it.
    DeleteFund(FundId),
    /// One row of a goal's allocation history. The goal's balance moves with
    /// it, and so does every figure derived from it.
    DeleteAllocation(AllocationId),
}

impl Confirm {
    /// The question the border asks, which is the one thing on screen that
    /// says which dialog this is.
    pub(super) fn title(self) -> &'static str {
        match self {
            Confirm::DeleteTxn(_) => "Delete this transaction?",
            Confirm::UndoBatch(_) => "Undo this batch?",
            Confirm::DeleteBill(_) => "Delete this bill?",
            Confirm::DeleteRecurringTxn(_) => "Delete this recurring transaction?",
            Confirm::DeleteRecurringGoal(_) => "Delete this recurring goal?",
            Confirm::DeleteFund(_) => "Delete this fund?",
            Confirm::DeleteAllocation(_) => "Delete this allocation?",
        }
    }

    /// What `y` does, in the dialog's own verb.
    ///
    /// The deletes share one arm rather than falling through a
    /// wildcard, so a new dialog that is neither a delete nor an undo is
    /// a compile error here instead of silently borrowing a verb that does
    /// not describe its write.
    pub(super) fn prompt(self) -> &'static str {
        match self {
            Confirm::UndoBatch(_) => "y undoes it · any other key cancels",
            Confirm::DeleteTxn(_)
            | Confirm::DeleteBill(_)
            | Confirm::DeleteRecurringTxn(_)
            | Confirm::DeleteRecurringGoal(_)
            | Confirm::DeleteFund(_)
            | Confirm::DeleteAllocation(_) => "y deletes · any other key cancels",
        }
    }

    /// The status line after any key but `y`. Listed the same way, and for
    /// the same reason, as [`Confirm::prompt`].
    pub(super) fn cancelled(self) -> &'static str {
        match self {
            Confirm::UndoBatch(_) => "undo cancelled",
            Confirm::DeleteTxn(_)
            | Confirm::DeleteBill(_)
            | Confirm::DeleteRecurringTxn(_)
            | Confirm::DeleteRecurringGoal(_)
            | Confirm::DeleteFund(_)
            | Confirm::DeleteAllocation(_) => "delete cancelled",
        }
    }

    /// The write `y` stands for, and what the status line says once it lands.
    ///
    /// Takes the `Db` rather than the whole `App` because that is all a
    /// delete needs: the caller is what reloads the screen behind it.
    pub(super) fn commit(self, db: &Db) -> Result<String> {
        Ok(match self {
            Confirm::DeleteTxn(id) => {
                txn::delete(db, id)?;
                "deleted".to_string()
            }
            Confirm::UndoBatch(id) => {
                goal::delete_batch(db, id)?;
                "batch undone".to_string()
            }
            Confirm::DeleteBill(id) => {
                bill::delete(db, id)?;
                "bill deleted".to_string()
            }
            Confirm::DeleteRecurringTxn(id) => {
                let released = recurring_txn::delete(db, id)?;
                format!("recurring transaction deleted · {released} rows released")
            }
            Confirm::DeleteRecurringGoal(id) => {
                recurring_goal::delete(db, id)?;
                "recurring goal deleted".to_string()
            }
            Confirm::DeleteFund(id) => {
                fund::delete(db, id)?;
                "fund deleted".to_string()
            }
            Confirm::DeleteAllocation(id) => {
                goal::delete_allocation(db, id)?;
                "allocation deleted".to_string()
            }
        })
    }
}

/// Draw whatever is open over the screen, and give back how many autocomplete
/// rows fitted.
///
/// That count is what `popup_key` may select: gated on what this draw fitted
/// rather than on the list being non-empty, so a popup clipped off the bottom
/// of a short terminal captures no keys. The viewports the three list modals
/// need come back out of their own draws for the same reason, which is why
/// this takes the modal by `&mut` and writes them back here.
pub(super) fn render(frame: &mut Frame, modal: &mut Option<Modal>, popup: &Autocomplete) -> usize {
    let mut viewport = None;
    let drawn = match &mut *modal {
        Some(Modal::Txn(f)) => ledger_form::render_txn(frame, f, popup),
        Some(Modal::Transfer(f)) => ledger_form::render_transfer(frame, f, popup),
        Some(Modal::Allocation(f)) => {
            goal_form::render_allocation(frame, f);
            0
        }
        Some(Modal::Goal(f)) => {
            goal_form::render_goal(frame, f);
            0
        }
        Some(Modal::GoalTransfer(f)) => {
            goal_form::render_goal_transfer(frame, f);
            0
        }
        Some(Modal::CloseOut(f)) => {
            goal_form::render_close(frame, f);
            0
        }
        Some(Modal::Worksheet(sheet)) => {
            viewport = Some(worksheet::render(frame, sheet));
            0
        }
        Some(Modal::Picker(p)) => {
            viewport = Some(picker::render(frame, p));
            0
        }
        Some(Modal::Destination(chooser)) => {
            viewport = Some(destination::render(frame, chooser));
            0
        }
        Some(Modal::Details(title, lines)) => {
            planning::render_details(frame, title, lines);
            0
        }
        Some(Modal::Confirm { action, label }) => {
            widget::render_fields(
                frame,
                action.title(),
                vec![
                    TextLine::from(label.clone()),
                    TextLine::from(""),
                    TextLine::from(action.prompt()),
                ],
            );
            0
        }
        Some(Modal::Value(_, f)) => {
            form::render_value(frame, f);
            0
        }
        Some(Modal::Bill(f)) => {
            planning::render_bill(frame, f);
            0
        }
        Some(Modal::RecurringTxn(f)) => recurring_txn_screen::render_form(frame, f, popup),
        Some(Modal::RecurringGoalEntry(f)) => {
            recurring_goal_screen::render_form(frame, f);
            0
        }
        Some(Modal::PlanTransfers(confirm)) => {
            planning::render_transfers(frame, confirm);
            0
        }
        Some(Modal::Fund(f)) => {
            fund_screen::render_form(frame, f);
            0
        }
        Some(Modal::Account(f)) => {
            accounts_screen::render_form(frame, f);
            0
        }
        // Each mode draws where the app already draws that shape: the form
        // and the dialog over the top of the list they were opened from.
        Some(Modal::History(h)) => {
            viewport = Some(history::render(frame, h));
            match h.mode_mut() {
                HistoryMode::List => {}
                HistoryMode::Editing(form) => goal_form::render_allocation(frame, form),
                HistoryMode::Confirming { action, label } => {
                    widget::render_fields(
                        frame,
                        action.title(),
                        vec![
                            TextLine::from(label.clone()),
                            TextLine::from(""),
                            TextLine::from(action.prompt()),
                        ],
                    );
                }
            }
            0
        }
        None => 0,
    };
    if let Some(viewport) = viewport {
        match modal {
            Some(Modal::Worksheet(sheet)) => sheet.record_viewport(viewport),
            Some(Modal::Picker(p)) => p.record_viewport(viewport),
            Some(Modal::Destination(chooser)) => chooser.record_viewport(viewport),
            Some(Modal::History(h)) => h.record_viewport(viewport),
            _ => {}
        }
    }
    drawn
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One dialog asks every one of these questions, so what is worth pinning is where
    /// they still differ: an undo is not a delete, and its border, its verb
    /// and its cancel all have to keep saying so.
    #[test]
    fn the_undo_dialog_keeps_its_own_words_and_the_deletes_keep_theirs() {
        let undo = Confirm::UndoBatch(BatchId(1));
        assert_eq!(undo.title(), "Undo this batch?");
        assert!(undo.prompt().contains("undoes it"), "{}", undo.prompt());
        assert_eq!(undo.cancelled(), "undo cancelled");

        for action in [
            Confirm::DeleteTxn(TxnId(1)),
            Confirm::DeleteBill(BillId(1)),
            Confirm::DeleteRecurringTxn(RecurringTxnId(1)),
            Confirm::DeleteRecurringGoal(RecurringGoalId(1)),
            Confirm::DeleteFund(FundId(1)),
            Confirm::DeleteAllocation(AllocationId(1)),
        ] {
            let title = action.title();
            assert!(title.starts_with("Delete this "), "{title}");
            assert!(title.ends_with('?'), "{title}");
            assert!(action.prompt().contains("y deletes"), "{title}");
            assert_eq!(action.cancelled(), "delete cancelled", "{title}");
        }
    }
}
