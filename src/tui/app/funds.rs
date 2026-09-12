//! Screen 6's key handling: adding, editing and deleting a holding, the
//! account filter and the search over the list, and `g`/`G`, which refresh a
//! fund's composition from SEC.
//!
//! No `Enter` here -- nothing yet draws a holding's long form, and a key
//! that does nothing is worse than a key that is absent.

use super::{Account, App, Deferred, NOTHING_SELECTED};
use crate::allocation::Class;
use crate::config::ADD_SEC_CONTACT;
use crate::db::account::{self, Kind};
use crate::db::fund_mix;
use crate::db::holding;
use crate::mix::{self, Refreshed};
use crate::rate::BasisPoints;
use crate::tui::cursor;
use crate::tui::fund::{HoldingForm, Row};
use crate::tui::modal::{Confirm, Modal};
use crate::tui::search::{self, Search};
use anyhow::{Context, Result};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;

impl App {
    pub(super) fn funds_key(&mut self, key: KeyEvent) -> Result<()> {
        if cursor::scroll_key(&mut self.funds, key.code) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if !search::escape_kept_filter(&mut self.funds) {
                    self.funds.clear_filters();
                }
            }
            KeyCode::Tab => self.funds.next_account(),
            KeyCode::BackTab => self.funds.previous_account(),
            KeyCode::Char('/') => self.funds.begin_search(),
            KeyCode::Char('a') => self.open_add_holding()?,
            KeyCode::Char('e') => self.open_edit_holding()?,
            KeyCode::Char('d') => self.open_delete_holding(),
            KeyCode::Char('g') => self.refresh_selected_mix()?,
            KeyCode::Char('G') => self.refresh_every_mix()?,
            _ => {}
        }
        Ok(())
    }

    /// Refreshes the selected row's ticker alone.
    fn refresh_selected_mix(&mut self) -> Result<()> {
        let Some(row) = self.funds.selected().cloned() else {
            return self.nothing_selected();
        };
        self.announce_refresh(vec![row.ticker])
    }

    /// Refreshes every ticker any holding names, not only the ones the
    /// account filter or a search is currently showing -- the same reading
    /// `mm mixes` takes, off `holding::tickers` rather than the rows on
    /// screen.
    fn refresh_every_mix(&mut self) -> Result<()> {
        let tickers = holding::tickers(&self.db)?;
        self.announce_refresh(tickers)
    }

    /// Says what is about to happen, and leaves the fetch itself to the
    /// event loop.
    ///
    /// [`Deferred`] carries why: the fetch blocks the loop, so a sentence set
    /// beside it would not be drawn until the fetch it describes was over.
    /// The two answers that involve no fetch are given here instead and are
    /// drawn on the next frame like any other status -- there is nothing to
    /// announce and nothing to wait for.
    fn announce_refresh(&mut self, tickers: Vec<String>) -> Result<()> {
        if self.sec_contact.is_none() {
            // `tui` cannot name the config file's path the way `mm mixes`
            // does: `sec_contact` reaches it as a bare `Option<String>` so
            // that this module need not name `config`.
            self.status =
                format!("no SEC contact configured -- {ADD_SEC_CONTACT} to the config file");
            return Ok(());
        }
        if tickers.is_empty() {
            // The sentence `mix::refresh` would reach anyway, taken from the
            // function that owns it rather than written a second time here.
            self.status = refresh_status(&Refreshed::default());
            return Ok(());
        }
        self.status = refreshing_status(&tickers);
        self.defer(Deferred::RefreshMixes(tickers));
        Ok(())
    }

    /// Blocks the event loop for as long as the fetches take: nothing in this
    /// crate is async and `mix::sec`'s calls are blocking. `mm mixes` is the
    /// route that does not tie up the screen; that is accepted here rather
    /// than solved, and what [`App::run_deferred`] adds is only that the
    /// screen says so while it happens.
    ///
    /// Reached only through [`Deferred::RefreshMixes`], which is why neither
    /// the contact nor the empty list is re-checked: [`App::announce_refresh`]
    /// answers both before anything is deferred at all.
    pub(super) fn refresh_mixes(&mut self, tickers: &[String]) -> Result<()> {
        let contact = self
            .sec_contact
            .clone()
            .context("a refresh was deferred with no SEC contact configured")?;
        let refreshed = mix::refresh(&self.db, &contact, tickers)?;
        self.status = refresh_status(&refreshed);
        self.reload()
    }

    /// Opens on the account the screen is filtered to, or on the first
    /// investment account when the filter is All.
    fn open_add_holding(&mut self) -> Result<()> {
        let accounts = account::list_by_kind(&self.db, Kind::Investment)?;
        let preselected = self.funds.filter_account();
        self.modal = Some(Modal::Holding(HoldingForm::add(accounts, preselected)?));
        Ok(())
    }

    fn open_edit_holding(&mut self) -> Result<()> {
        let Some(row) = self.funds.selected().cloned() else {
            return self.nothing_selected();
        };
        let accounts = account::list_by_kind(&self.db, Kind::Investment)?;
        self.modal = Some(Modal::Holding(HoldingForm::edit(accounts, &row)?));
        Ok(())
    }

    fn open_delete_holding(&mut self) {
        match self.funds.selected().cloned() {
            None => self.status = NOTHING_SELECTED.to_string(),
            Some(row) => {
                let label = format!(
                    "{}  {}  {}",
                    crate::demo::text(&row.ticker),
                    crate::demo::text(self.funds.account_name(row.account_id)),
                    crate::demo::whole_figure(row.balance)
                );
                self.modal = Some(Modal::Confirm {
                    action: Confirm::DeleteHolding(row.id),
                    label,
                });
            }
        }
    }

    pub(super) fn commit_holding_form(&mut self) -> Result<()> {
        let Some(Modal::Holding(form)) = &self.modal else {
            return Ok(());
        };
        let (account_id, ticker, balance) = form.commit()?;
        let verb = match form.editing {
            Some(id) => {
                holding::update(&self.db, id, account_id, &ticker, balance)?;
                "updated"
            }
            None => {
                holding::insert(&self.db, account_id, &ticker, balance)?;
                "added"
            }
        };
        self.status = format!(
            "{verb} {} {}",
            crate::demo::text(&ticker),
            crate::demo::whole_figure(balance)
        );
        self.close_modal();
        self.reload()
    }

    pub(super) fn reload_funds(&mut self) -> Result<()> {
        let accounts = account::list_by_kind(&self.db, Kind::Investment)?;
        self.funds.set_accounts(accounts.clone());
        self.funds
            .set_targets(crate::fund::targets_from_db(&self.db, self.today)?);

        let holdings = holding::list(&self.db)?;
        let mut mixes: HashMap<String, fund_mix::Mix> = HashMap::new();
        for ticker in holding::tickers(&self.db)? {
            if let Some(mix) = fund_mix::for_ticker(&self.db, &ticker)? {
                mixes.insert(ticker, mix);
            }
        }
        // The summary reads the whole composition where a row reads only its
        // stock share, so the slices go in beside the rows rather than onto
        // them -- a fund is one composition however many accounts hold it.
        self.funds.set_mixes(
            mixes
                .iter()
                .map(|(ticker, mix)| (ticker.clone(), mix.slices.clone()))
                .collect(),
        );

        let rows = holdings
            .into_iter()
            .map(|h| {
                let mix = mixes.get(&h.ticker);
                Row {
                    id: h.id,
                    account_id: h.account_id,
                    account: Account::named(&accounts, h.account_id),
                    ticker: h.ticker,
                    // Cased here rather than in the cell: what the screen
                    // filters and draws is one string, so a needle matches
                    // the words a reader can see.
                    name: mix
                        .and_then(|m| m.name.as_deref())
                        .map(crate::fund_label::short),
                    balance: h.balance,
                    stock_percent: mix.map(stock_share),
                    as_of: mix.map(|m| m.report_date),
                    tax_treatment: accounts
                        .iter()
                        .find(|a| a.id == h.account_id)
                        .and_then(|a| a.tax_treatment),
                }
            })
            .collect();
        self.funds.set_rows(rows);
        Ok(())
    }
}

/// The stock share of a fund's composition -- U.S. plus international --
/// out of its published slices.
///
/// Through [`Class`] rather than by naming the two `AssetClass` variants
/// here. Which asset classes count as stock is that enum's to say, and the
/// summary and the bar one panel up already ask it: a second answer at this
/// column is one the `Stock%` beside a row could give while the panel above
/// gave the other.
fn stock_share(mix: &fund_mix::Mix) -> BasisPoints {
    Class::UsStock.actual(&mix.slices) + Class::IntlStock.actual(&mix.slices)
}

/// What the status line says while a refresh has the screen frozen.
///
/// One ticker is named, several are counted. `g` acts on the row under the
/// cursor and naming it is what confirms which row that was; `G` over a
/// dozen funds would spend the line on a list nobody reads while they wait,
/// and the count is the part that says how long this is going to take.
///
/// Masked through `crate::demo::text` for [`refresh_status`]'s reason, and
/// it ends in an ellipsis for a reason of its own: it is the one message in
/// the app that describes something still happening rather than something
/// that has happened.
fn refreshing_status(tickers: &[String]) -> String {
    match tickers {
        [ticker] => format!("refreshing {}...", crate::demo::text(ticker)),
        many => format!("refreshing {} funds...", many.len()),
    }
}

/// What `g`/`G` leave on the status line: honest about a run that updated
/// some tickers and failed others, rather than reporting success on the
/// strength of `updated` alone. Every ticker is masked through
/// `crate::demo::text` on the way out, the same as every other name a
/// screen draws.
fn refresh_status(refreshed: &Refreshed) -> String {
    let updated = || {
        refreshed
            .updated
            .iter()
            .map(|t| crate::demo::text(t))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let failed = || {
        refreshed
            .failed
            .iter()
            .map(|(t, e)| format!("{}: {e}", crate::demo::text(t)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    match (refreshed.updated.is_empty(), refreshed.failed.is_empty()) {
        (true, true) => "nothing to refresh".to_string(),
        (false, true) => format!("refreshed {}", updated()),
        (true, false) => format!("failed to refresh {}", failed()),
        (false, false) => format!("refreshed {}; failed to refresh {}", updated(), failed()),
    }
}

#[cfg(test)]
mod tests {
    use super::{Refreshed, refresh_status, refreshing_status, stock_share};
    use crate::allocation::{self, Class};
    use crate::db::fund_mix::{self, AssetClass, Slice};
    use crate::db::setting::{self, key};
    use crate::money::Cents;
    use crate::rate::BasisPoints;
    use crate::test_support::day;
    use crate::tui::app::test_support;
    use crate::tui::modal::Modal;
    use ratatui::crossterm::event::KeyCode;

    /// A refresh with no failures says only what was updated.
    #[test]
    fn a_fully_successful_refresh_names_every_updated_ticker() {
        let refreshed = Refreshed {
            updated: vec!["USM".to_string(), "USB".to_string()],
            failed: vec![],
        };
        assert_eq!(refresh_status(&refreshed), "refreshed USM, USB");
    }

    /// A run where some tickers updated and some failed is reported as one:
    /// the status line must carry both halves rather than reading as a
    /// success on the strength of `updated` alone.
    #[test]
    fn a_partial_refresh_names_both_what_updated_and_what_failed() {
        let refreshed = Refreshed {
            updated: vec!["USM".to_string()],
            failed: vec![("ISM".to_string(), "throttled".to_string())],
        };
        assert_eq!(
            refresh_status(&refreshed),
            "refreshed USM; failed to refresh ISM: throttled"
        );
    }

    /// Every ticker failing is still reported, not silently swallowed as
    /// "nothing to refresh" -- that phrase is reserved for an empty list.
    #[test]
    fn a_refresh_where_everything_failed_names_every_failure() {
        let refreshed = Refreshed {
            updated: vec![],
            failed: vec![
                ("USM".to_string(), "throttled".to_string()),
                (
                    "ISM".to_string(),
                    "SEC lists no series for ticker".to_string(),
                ),
            ],
        };
        assert_eq!(
            refresh_status(&refreshed),
            "failed to refresh USM: throttled; ISM: SEC lists no series for ticker"
        );
    }

    /// Reachable only if `G` is pressed with no holdings in the database at
    /// all -- an empty ticker list is neither an update nor a failure, so it
    /// earns its own phrase rather than falling into either of the arms
    /// above.
    #[test]
    fn a_refresh_of_nothing_says_there_was_nothing_to_refresh() {
        let refreshed = Refreshed {
            updated: vec![],
            failed: vec![],
        };
        assert_eq!(refresh_status(&refreshed), "nothing to refresh");
    }

    #[test]
    fn a_holding_with_no_mix_on_record_reads_as_never_fetched() {
        let app = test_support::app_with_holdings();

        let rows = app.funds.rows();
        let row = rows.first().expect("a holding is listed");
        assert_eq!(row.as_of, None);
        assert_eq!(
            row.stock_percent, None,
            "a fund with no mix reported a stock share"
        );
    }

    #[test]
    fn the_account_filter_narrows_the_list_to_one_account() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let all = app.funds.rows().len();

        test_support::press(&mut app, KeyCode::Tab);

        let filtered = app.funds.rows().len();
        assert!(filtered < all, "Tab did not narrow the list");
        assert!(
            app.funds
                .rows()
                .iter()
                .all(|r| r.account_id == app.funds.filter_account().unwrap()),
            "a row from another account survived the filter"
        );
    }

    #[test]
    fn back_tab_narrows_the_list_from_the_other_direction() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let all = app.funds.rows().len();

        test_support::press(&mut app, KeyCode::BackTab);

        let filtered = app.funds.rows().len();
        assert!(filtered < all, "BackTab did not narrow the list");
        assert_eq!(
            app.funds.filter_account(),
            app.funds.rows().first().map(|r| r.account_id)
        );
    }

    /// `Esc` clears a kept search before it touches the account filter, the
    /// same order Ledger and Savings answer it in -- both narrow the screen
    /// two ways, and a press that cleared only one would leave the owner to
    /// work out which is still hiding a row.
    #[test]
    fn esc_clears_a_kept_search_before_the_account_filter() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        test_support::press(&mut app, KeyCode::Tab);
        assert_eq!(app.funds.rows().len(), 2, "BRK holds USM and USB");

        test_support::press(&mut app, KeyCode::Char('/'));
        test_support::type_str(&mut app, "USB");
        test_support::press(&mut app, KeyCode::Enter);
        assert_eq!(app.funds.rows().len(), 1, "the kept search did not narrow");

        test_support::press(&mut app, KeyCode::Esc);
        assert!(
            app.funds.filter_account().is_some(),
            "the first Esc must leave the account filter alone"
        );
        assert_eq!(
            app.funds.rows().len(),
            2,
            "the first Esc must clear only the search"
        );

        test_support::press(&mut app, KeyCode::Esc);
        assert_eq!(app.funds.filter_account(), None);
        assert_eq!(
            app.funds.rows().len(),
            3,
            "the second Esc must clear to All"
        );
    }

    /// `a` opens on the account the `Tab` filter names, and a committed
    /// holding reaches the list without a second reload.
    #[test]
    fn pressing_a_adds_a_holding_to_the_list() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        test_support::press(&mut app, KeyCode::Tab);
        let filtered_account = app.funds.filter_account().unwrap();

        test_support::press(&mut app, KeyCode::Char('a'));
        test_support::type_str(&mut app, "UNC");
        test_support::press(&mut app, KeyCode::Tab);
        test_support::type_str(&mut app, "2000");
        test_support::press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        let rows = app.funds.rows();
        let added = rows
            .iter()
            .find(|r| r.ticker == "UNC")
            .expect("the new holding is listed");
        assert_eq!(added.balance, Cents::from_dollars(2_000));
        assert_eq!(
            added.account_id, filtered_account,
            "a did not open on the account the Tab filter named"
        );
    }

    /// A ticker is a key in three places and none of them folds case, so the
    /// form uppercases what is typed. Without that, `usm` and `USM` are two
    /// holdings in one account and two independent compositions, and a mix
    /// fetched under one spelling never reaches a holding typed in the other.
    #[test]
    fn typing_a_lowercase_ticker_stores_it_uppercase() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));

        test_support::press(&mut app, KeyCode::Char('a'));
        test_support::type_str(&mut app, "unc");
        test_support::press(&mut app, KeyCode::Tab);
        test_support::type_str(&mut app, "2000");
        test_support::press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the form stayed open: {}", app.status);
        assert!(
            app.funds.rows().iter().any(|r| r.ticker == "UNC"),
            "the ticker was not stored uppercase: {:?}",
            app.funds
                .rows()
                .iter()
                .map(|r| &r.ticker)
                .collect::<Vec<_>>()
        );
    }

    /// The other half of the same rule, from the direction it protects: the
    /// fixture already holds `USM`, so a lowercase re-entry into that account
    /// must meet the duplicate guard rather than open a second row under a
    /// second spelling.
    #[test]
    fn a_lowercase_re_entry_of_a_held_ticker_is_refused_as_a_duplicate() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        test_support::press(&mut app, KeyCode::Tab);
        let account = app.funds.filter_account().unwrap();
        let before = app.funds.rows().len();

        test_support::press(&mut app, KeyCode::Char('a'));
        test_support::type_str(&mut app, "usm");
        test_support::press(&mut app, KeyCode::Tab);
        test_support::type_str(&mut app, "2000");
        test_support::press(&mut app, KeyCode::Enter);

        assert!(
            app.modal.is_some(),
            "a duplicate ticker was accepted: {}",
            app.status
        );
        assert!(!app.status.is_empty(), "the refusal said nothing");
        test_support::press(&mut app, KeyCode::Esc);
        assert_eq!(
            app.funds.rows().len(),
            before,
            "a second row was written under a second spelling"
        );
        assert_eq!(
            app.funds
                .rows()
                .iter()
                .filter(|r| r.account_id == account && r.ticker.eq_ignore_ascii_case("USM"))
                .count(),
            1
        );
    }

    /// `e` opens on the ticker like `a` does; `Tab` reaches the balance,
    /// which is the field most worth checking on an existing row.
    #[test]
    fn pressing_e_edits_the_selected_holdings_balance() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let before = app.funds.rows()[0].ticker.clone();

        test_support::press(&mut app, KeyCode::Char('e'));
        test_support::press(&mut app, KeyCode::Tab);
        for _ in 0.."10,000.00".len() {
            test_support::press(&mut app, KeyCode::Backspace);
        }
        test_support::type_str(&mut app, "12000");
        test_support::press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the edit stayed open: {}", app.status);
        let rows = app.funds.rows();
        let edited = rows.iter().find(|r| r.ticker == before).unwrap();
        assert_eq!(edited.balance, Cents::from_dollars(12_000));
    }

    /// The bug this guards: `e` opens on `Ticker`, so reaching `Account`
    /// takes a `BackTab` first. Before the account was threaded through
    /// `commit_holding_form`, cycling it here and pressing `Enter` reported
    /// `updated` over a row that had not moved.
    #[test]
    fn pressing_e_can_move_a_holding_to_another_account() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let moving = app.funds.rows()[0].clone();

        test_support::press(&mut app, KeyCode::Char('e'));
        test_support::press(&mut app, KeyCode::BackTab);
        test_support::press(&mut app, KeyCode::Right);
        test_support::press(&mut app, KeyCode::Enter);

        assert!(app.modal.is_none(), "the edit stayed open: {}", app.status);
        let rows = app.funds.rows();
        let after = rows.iter().find(|r| r.ticker == moving.ticker).unwrap();
        assert_ne!(
            after.account_id, moving.account_id,
            "the account was not moved"
        );
    }

    #[test]
    fn pressing_d_deletes_the_selected_holding_after_confirming() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let before = app.funds.rows().len();
        let deleting = app.funds.rows()[0].ticker.clone();

        test_support::press(&mut app, KeyCode::Char('d'));
        assert!(
            matches!(app.modal, Some(Modal::Confirm { .. })),
            "no confirmation opened"
        );
        test_support::press(&mut app, KeyCode::Char('y'));

        assert!(app.modal.is_none());
        let rows = app.funds.rows();
        assert_eq!(rows.len(), before - 1);
        assert!(!rows.iter().any(|r| r.ticker == deleting));
    }

    /// Any other key cancels a confirmation, the same as every delete
    /// dialog in the app.
    #[test]
    fn cancelling_a_delete_confirmation_leaves_the_holding_in_place() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        let before = app.funds.rows().len();

        test_support::press(&mut app, KeyCode::Char('d'));
        test_support::press(&mut app, KeyCode::Esc);

        assert!(app.modal.is_none());
        assert_eq!(app.funds.rows().len(), before);
    }

    /// A search narrowed to nothing leaves the cursor with no row to act
    /// on, and `e`/`d` say so rather than opening on whatever the cursor's
    /// stale index would otherwise land on.
    #[test]
    fn e_and_d_with_nothing_selected_say_nothing_selected() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));
        test_support::press(&mut app, KeyCode::Char('/'));
        test_support::type_str(&mut app, "zzz");
        test_support::press(&mut app, KeyCode::Enter);
        assert!(app.funds.rows().is_empty(), "the needle matched something");

        test_support::press(&mut app, KeyCode::Char('e'));
        assert_eq!(app.status, "nothing selected");
        assert!(app.modal.is_none());

        test_support::press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.status, "nothing selected");
        assert!(app.modal.is_none());
    }

    /// `stock_share` sums the two stock classes and leaves the rest out --
    /// a bond-heavy target-date fund still reads its own stock share rather
    /// than the whole of its published composition.
    #[test]
    fn the_stock_share_is_the_us_and_international_slices_and_nothing_else() {
        let mix = fund_mix::Mix {
            ticker: "TDF45".to_string(),
            name: Some(crate::test_support::fund_name("TDF45").to_string()),
            report_date: day(2026, 6, 30),
            slices: vec![
                Slice {
                    class: AssetClass::UsStock,
                    weight: BasisPoints(4_500),
                },
                Slice {
                    class: AssetClass::IntlStock,
                    weight: BasisPoints(3_000),
                },
                Slice {
                    class: AssetClass::UsBond,
                    weight: BasisPoints(1_500),
                },
                Slice {
                    class: AssetClass::Cash,
                    weight: BasisPoints(1_000),
                },
            ],
        };
        assert_eq!(stock_share(&mix), BasisPoints(7_500));
    }

    /// `mix::refresh` always reaches the network, so this crate's tests
    /// must never call it with a contact configured -- `app_with_holdings`
    /// carries none, which is what lets this run offline and still exercise
    /// the refusal.
    #[test]
    fn refreshing_with_no_sec_contact_configured_says_what_to_set() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));

        test_support::press(&mut app, KeyCode::Char('G'));

        assert!(
            app.status.contains("contact"),
            "the refusal does not name the setting to fix: {}",
            app.status
        );
    }

    /// The announcement is on screen *before* the fetch blocks the loop,
    /// which is the whole of what the deferral buys: `G` leaves a sentence
    /// and a piece of work, and the loop draws the first before running the
    /// second.
    ///
    /// Asserted without a contact configured, so nothing here reaches the
    /// network -- `app_with_holdings` carries none, and what is being pinned
    /// is the order, not the fetch.
    #[test]
    fn a_refresh_announces_itself_and_leaves_the_fetch_for_the_loop() {
        let mut app = test_support::app_with_holdings();
        app.sec_contact = Some("someone@example.com".to_string());
        test_support::press(&mut app, KeyCode::Char('6'));

        test_support::press(&mut app, KeyCode::Char('G'));

        assert!(
            app.status.starts_with("refreshing"),
            "the freeze is not announced: {}",
            app.status
        );
        assert!(
            app.has_deferred(),
            "the fetch ran inside the key handler, with no frame drawn first"
        );
    }

    /// `g` names the one row it acts on, where `G` counts them: the count is
    /// what says how long the screen is about to be frozen for, and a list of
    /// a dozen tickers is not read by somebody waiting.
    #[test]
    fn one_ticker_is_named_in_the_announcement_and_several_are_counted() {
        assert_eq!(refreshing_status(&["USM".to_string()]), "refreshing USM...");
        assert_eq!(
            refreshing_status(&["USM".to_string(), "USB".to_string()]),
            "refreshing 2 funds..."
        );
    }

    /// Neither answer involves a fetch, so neither is deferred: a refusal
    /// that waited a frame to be drawn beside work that was never going to
    /// run would be the announcement mechanism used to say nothing.
    #[test]
    fn a_refresh_with_nothing_to_fetch_defers_no_work() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));

        test_support::press(&mut app, KeyCode::Char('G'));
        assert!(app.status.contains("contact"), "{}", app.status);
        assert!(!app.has_deferred(), "a refusal was deferred");
    }

    /// The other key of the pair: `g` refuses the same way over the
    /// selected row alone, rather than silently doing nothing because
    /// nothing is selected.
    #[test]
    fn refreshing_the_selected_row_with_no_sec_contact_configured_says_what_to_set() {
        let mut app = test_support::app_with_holdings();
        test_support::press(&mut app, KeyCode::Char('6'));

        test_support::press(&mut app, KeyCode::Char('g'));

        assert!(
            app.status.contains("contact"),
            "the refusal does not name the setting to fix: {}",
            app.status
        );
    }

    /// The end-to-end reading: a mix on record for one holding's ticker
    /// reaches the row as a stock share and the filing date it was read
    /// off, while the other holding's ticker -- never fetched -- stays
    /// `None`.
    #[test]
    fn reload_prices_a_holding_whose_ticker_has_a_mix_on_record() {
        let mut app = test_support::app_with_holdings();
        let report_date = day(2026, 6, 30);
        fund_mix::set_for_ticker(
            &app.db,
            "USM",
            report_date,
            Some(crate::test_support::fund_name("USM")),
            &[Slice {
                class: AssetClass::UsStock,
                weight: BasisPoints(10_000),
            }],
        )
        .unwrap();
        app.reload().unwrap();

        let rows = app.funds.rows();
        let priced = rows.iter().find(|r| r.ticker == "USM").unwrap();
        assert_eq!(priced.stock_percent, Some(BasisPoints(10_000)));
        assert_eq!(priced.as_of, Some(report_date));

        let unpriced = rows.iter().find(|r| r.ticker == "USB").unwrap();
        assert_eq!(unpriced.stock_percent, None);
        assert_eq!(unpriced.as_of, None);
    }

    /// The look-through is a share of what it covers, so whatever the mixes
    /// carry it foots to a whole hundred percent.
    #[test]
    fn the_summary_totals_every_holding_weighted_by_its_mix() {
        let mut app = test_support::app_with_mixes();
        test_support::press(&mut app, KeyCode::Char('6'));

        let summary = app.funds.summary();
        let total: i64 = summary.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, 10_000, "the summary does not foot to 100%");
    }

    /// The residual keeps its slice whatever it holds -- `Class::Other`
    /// draws it beside the cash, so a portfolio with nothing unplaced reports
    /// a zero rather than a missing class.
    #[test]
    fn the_residual_keeps_its_zero_when_nothing_is_unclassified() {
        let mut app = test_support::app_with_mixes();
        test_support::press(&mut app, KeyCode::Char('6'));

        let summary = app.funds.summary();
        assert!(
            summary.iter().any(|s| s.class == AssetClass::Unclassified
                && s.weight == crate::rate::BasisPoints::ZERO),
            "the residual lost its slice: {summary:?}"
        );
    }

    /// "Where do the bonds live" is answered by the filter the screen already
    /// had: `BRK` holds the bond fund and `RET` the international one, so the
    /// two narrowings cannot come to the same summary.
    #[test]
    fn the_account_filter_recomputes_the_summary_for_that_account_alone() {
        let mut app = test_support::app_with_mixes();
        test_support::press(&mut app, KeyCode::Char('6'));
        let all = app.funds.summary();

        test_support::press(&mut app, KeyCode::Tab);

        assert_ne!(
            app.funds.summary(),
            all,
            "Tab did not recompute the summary"
        );
    }

    /// The age rule produces one bond number, so the row it is drawn against
    /// is one bond figure: the two bond classes added, not whichever of them
    /// happens to be larger.
    #[test]
    fn the_bond_target_is_drawn_against_the_combined_bond_share() {
        let mut app = test_support::app_with_mixes();
        test_support::press(&mut app, KeyCode::Char('6'));

        let bonds = app.funds.summary_row(Class::Bonds).expect("a Bonds row");
        let summary = app.funds.summary();
        assert_eq!(
            bonds.actual,
            allocation::weight(&summary, AssetClass::UsBond)
                + allocation::weight(&summary, AssetClass::IntlBond)
        );
        assert_ne!(
            allocation::weight(&summary, AssetClass::IntlBond),
            BasisPoints::ZERO,
            "a fixture with only one bond class would pass either way"
        );
    }

    /// No birth date is a question rather than a zero, all the way to the
    /// cell: a `0.00%` bond target reads as advice nobody gave.
    #[test]
    fn with_no_birth_date_on_record_the_bond_target_is_blank_rather_than_zero() {
        let mut app = test_support::app_with_mixes();
        setting::clear(&app.db, key::BIRTH_DATE).unwrap();
        test_support::press(&mut app, KeyCode::Char('6'));
        app.reload().unwrap();

        let bonds = app.funds.summary_row(Class::Bonds).unwrap();
        assert_eq!(
            bonds.target, None,
            "a missing birth date drew a zero bond target"
        );
        assert_eq!(bonds.delta, None);
    }
}
