//! The standing HTML report: what the screens show, as one page a phone can
//! open -- the Overview, both ledgers, Savings, Planning and Funds, each a tab
//! of it.
//!
//! `html` writes the page readably and `write` minifies it on the way to the
//! disk, so what a test asserts against and what a phone downloads are the
//! same page in two forms.

pub mod html;

use crate::account_label::Account;
use crate::allocation::{Class, Held, SummaryRow};
use crate::calc;
use crate::calc::planning::PlanSettings;
use crate::db::account::Kind;
use crate::db::{Db, account, bill, fund_mix, holding, txn};
use crate::goal as goal_engine;
use crate::money::Cents;
use crate::overview::Overview;
use crate::plan;
use crate::plan_rows;
use crate::projection;
use crate::reading::Reading;
use crate::savings;
use crate::transfer;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};
use minify_html::Cfg;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One container's goals and its unallocated remainder.
pub struct Container {
    pub account: Account,
    pub rows: Vec<savings::Row>,
    pub excess: Cents,
}

/// The portfolio's look-through, the age rule it is read against, and the
/// same pair again per investment account.
///
/// `crate::allocation::Allocation` is the apportioning itself and this is
/// what a tab needs beside it: the targeted rows already resolved, and the
/// accounts stacked, since a page has no `Tab` to cycle one at a time with.
pub struct Allocation {
    /// Every holding in every investment account, apportioned by class.
    pub lookthrough: crate::allocation::Allocation,
    /// The three targeted classes as `Target`/`Actual`/`Δ`, or nothing at
    /// all when there is no composition to read them against -- the state
    /// every database starts in, and the one the tab answers with a sentence
    /// rather than a table of dashes.
    pub summary: Vec<SummaryRow>,
    /// One section per investment account holding something, in
    /// `account::list_by_kind`'s order.
    ///
    /// An account holding nothing is left out. The screen's `Tab` cycle
    /// still reaches it, because a filter has to be able to say "this one",
    /// but a heading over an empty table costs a phone a screen height to
    /// say the same.
    pub accounts: Vec<AccountAllocation>,
}

/// One investment account's own look-through, and the holdings behind it.
///
/// Both halves, because `Tab` on the screen narrows the summary and the list
/// together: a section carrying only the list would be a spelling of half of
/// what the screen does.
pub struct AccountAllocation {
    pub account: Account,
    pub lookthrough: crate::allocation::Allocation,
    pub summary: Vec<SummaryRow>,
    pub holdings: Vec<Holding>,
}

/// One holding, as the Funds tab lists it: `tui::fund::Row`'s columns less
/// the account, which the section heading already names.
pub struct Holding {
    pub ticker: String,
    pub balance: Cents,
    /// The filing this fund's composition was read out of, and `None`
    /// exactly when there is no composition on record -- which is also
    /// exactly what puts the holding outside the summary above it. One
    /// column says both because they are one fact.
    pub as_of: Option<NaiveDate>,
}

/// One ledger's rows, grouped into the months the page's filter switches
/// between.
///
/// Every row there is, where the screen shows a month or two: a page is
/// scrolled and filtered rather than paged through, and a report that stopped
/// at the current window would be missing exactly what someone reaches for it
/// to check.
pub struct Ledger {
    pub kind: Kind,
    pub months: Vec<LedgerMonth>,
    /// The month `today` falls in, as a `months` key. Not necessarily one of
    /// them: a ledger with nothing yet entered this month has no such group,
    /// and the filter falls back to showing all of them.
    pub current: String,
}

/// One month of one ledger.
pub struct LedgerMonth {
    /// `2026-08`. What the filter addresses this month by, never shown.
    pub key: String,
    /// `Aug 2026`, what the dropdown shows -- `tui::ledger::Window::label`'s
    /// wording for a single-month window.
    pub label: String,
    pub rows: Vec<LedgerRow>,
}

/// A transaction as the ledger screen draws it: date, account, description,
/// amount, and whether it is still in the future.
pub struct LedgerRow {
    pub date: NaiveDate,
    pub account: Account,
    pub description: String,
    /// As stored -- cash signed naturally, credit signed as debt, the way
    /// each screen matches its own sheet. Only the Overview negates.
    pub cents: Cents,
    pub future: bool,
}

/// The Planning screen's figures, or the reason there are none.
///
/// A plan that cannot resolve is an ordinary state -- a database with no
/// account in the `Checking` band has one -- and the screen renders the
/// message in place of the waterfall. The page does the same rather than
/// failing the whole report over one tab.
pub enum Planning {
    Unresolvable(String),
    Resolved(Box<PlanView>),
}

pub struct PlanView {
    pub settings: PlanSettings,
    pub plan: crate::calc::planning::Plan,
    pub housing: Vec<plan_rows::Bill>,
    pub other_bills: Vec<plan_rows::Bill>,
    /// The transfers, or why they cannot be resolved. Separate from the
    /// waterfall above: a dangling destination key stops the money moving
    /// without making any figure above it wrong.
    pub transfers: Result<Vec<plan_rows::Transfer>, String>,
    /// What the goals the plug spreads over ask of this paycheck, summed.
    /// The figure the Goals line is measured against, and the same one the
    /// Planning screen measures it against.
    pub spread_ask_total: Cents,
    /// The waterfall constants the owner has marked as biweekly expenses.
    pub expense_constants: Vec<plan_rows::Target>,
}

/// Everything the report draws, read in one pass.
pub struct Snapshot {
    /// Wall time, not `--today`. How old the page is, is a fact about the
    /// clock -- the same reason the backup schedule reads `Utc::now`.
    pub generated_at: DateTime<Local>,
    pub overview: Overview,
    pub cash: Ledger,
    pub credit: Ledger,
    pub containers: Vec<Container>,
    pub planning: Planning,
    pub allocation: Allocation,
}

/// One ledger, every row of it, grouped by month.
///
/// The grouping leans on `txn::list` returning rows in date order: a month
/// ends where the next key differs, so nothing sorts twice.
fn ledger(db: &Db, accounts: &[account::Account], kind: Kind, today: NaiveDate) -> Result<Ledger> {
    let current = today.format("%Y-%m").to_string();
    let Some((from, to)) = txn::date_range(db)? else {
        return Ok(Ledger {
            kind,
            months: Vec::new(),
            current,
        });
    };
    let filter = txn::Filter {
        kind,
        account_id: None,
        from,
        to,
    };
    let mut months: Vec<LedgerMonth> = Vec::new();
    for t in txn::list(db, &filter)? {
        let key = t.date.format("%Y-%m").to_string();
        if months.last().is_none_or(|m| m.key != key) {
            months.push(LedgerMonth {
                label: t.date.format("%b %Y").to_string(),
                key,
                rows: Vec::new(),
            });
        }
        months
            .last_mut()
            .expect("a month was just pushed if there was none")
            .rows
            .push(LedgerRow {
                date: t.date,
                account: Account::named(accounts, t.account_id),
                description: t.description,
                cents: t.cents,
                future: t.date > today,
            });
    }
    Ok(Ledger {
        kind,
        months,
        current,
    })
}

/// The three targeted classes over one look-through, or none at all when
/// there is nothing to look through.
///
/// Absent rather than zeroed, exactly as `tui::fund::Funds::summary_row` is
/// absent: a portfolio nobody has fetched a mix for holds an *unknown* share
/// of bonds, and a table of zeroes against the age rule's targets would
/// report it as the worst-allocated portfolio on record.
fn summary(
    lookthrough: &crate::allocation::Allocation,
    targets: calc::fund::Targets,
) -> Vec<SummaryRow> {
    if lookthrough.slices.is_empty() {
        return Vec::new();
    }
    Class::ALL
        .iter()
        .map(|class| SummaryRow::new(*class, &lookthrough.slices, targets))
        .collect()
}

/// Every investment account's holdings, apportioned by class -- each account
/// on its own, and all of them together.
///
/// The whole portfolio is apportioned over the same holdings the sections
/// are, rather than summed out of them: a weighted average of six shares is
/// the apportioning again in a form that can round differently, and one
/// portfolio may not have two compositions.
///
/// The mixes are read once, by ticker, for the reason the screen reads them
/// that way: a fund's composition is a property of the fund, so one fetch
/// prices every account holding it.
fn allocation_view(db: &Db, today: NaiveDate, accounts: &[account::Account]) -> Result<Allocation> {
    let targets = crate::fund::targets_from_db(db, today)?;
    let mut mixes: HashMap<String, fund_mix::Mix> = HashMap::new();
    for ticker in holding::tickers(db)? {
        if let Some(mix) = fund_mix::for_ticker(db, &ticker)? {
            mixes.insert(ticker, mix);
        }
    }

    let mut sections = Vec::new();
    let mut portfolio: Vec<Held<'_>> = Vec::new();
    for account in account::list_by_kind(db, Kind::Investment)? {
        let holdings = holding::list_for_account(db, account.id)?;
        if holdings.is_empty() {
            continue;
        }
        let held: Vec<Held<'_>> = holdings
            .iter()
            .map(|h| Held {
                balance: h.balance,
                treatment: account.tax_treatment,
                mix: mixes.get(&h.ticker).map(|m| m.slices.as_slice()),
            })
            .collect();
        portfolio.extend(held.iter().copied());
        let lookthrough = crate::allocation::apportion(&held);
        sections.push(AccountAllocation {
            account: Account::named(accounts, account.id),
            summary: summary(&lookthrough, targets),
            lookthrough,
            holdings: holdings
                .into_iter()
                .map(|h| Holding {
                    as_of: mixes.get(&h.ticker).map(|m| m.report_date),
                    ticker: h.ticker,
                    balance: h.balance,
                })
                .collect(),
        });
    }

    let lookthrough = crate::allocation::apportion(&portfolio);
    Ok(Allocation {
        summary: summary(&lookthrough, targets),
        lookthrough,
        accounts: sections,
    })
}

/// The waterfall, its bills, and the transfers it would write.
///
/// `adhoc` and not `App::adhoc`: `Excess (Actual)` *is* the checking balance
/// at that date, so the figure here has to be quoted at the same day the
/// Overview's Paycheck-Eve column is.
fn plan_view(db: &Db, today: NaiveDate, adhoc: NaiveDate) -> Result<PlanView> {
    let settings = plan::settings_from_db(db)?;
    let plan = plan::compute_from_db(db, &settings, adhoc)?;
    let periods = settings.periods_per_year;
    let bills = |category| -> Result<Vec<plan_rows::Bill>> {
        bill::list(db, category)?
            .into_iter()
            .map(|b| {
                Ok(plan_rows::Bill {
                    id: b.id,
                    label: b.label,
                    monthly: b.cents,
                    biweekly: calc::biweekly(b.cents, periods)?,
                    counts_as_expense: b.counts_as_expense,
                })
            })
            .collect()
    };
    // The asks are read on their own, exactly as `App::planning_view` reads
    // them and for the reason it does: the payday `Unmet Asks` exists for is
    // the one where every line is zero, which is the payday `transfer::plan`
    // refuses outright, so asks chained to that call would go silent on the
    // one state they are drawn outside the block to reach.
    //
    // Not propagated, because a failure here is an annotation that cannot be
    // made rather than a tab that cannot be drawn: a taxed goal with no rate
    // on record trips the strict target reader and would take the whole tab
    // down -- and it is reachable, since `plan` never touches that reader
    // when the plug is zero and so returns `Ok` where this call does not.
    // The read just made, handed on rather than made again: `plan` resolves
    // the plug's container off exactly these goals. A read that failed hands
    // over nothing and leaves `plan` to fail on its own terms.
    let asks = transfer::spread_asks(db, today, periods);
    let spread_ask_total = asks
        .as_ref()
        .map(transfer::Asks::total)
        .unwrap_or(Cents::ZERO);
    let transfers = match transfer::plan(db, &plan.lines, asks.as_ref().ok()) {
        Ok(rows) => Ok(plan_rows::transfers(&rows)),
        Err(e) => Err(format!("{e:#}")),
    };
    Ok(PlanView {
        housing: bills(bill::Category::Housing)?,
        other_bills: bills(bill::Category::Other)?,
        settings,
        plan,
        transfers,
        spread_ask_total,
        expense_constants: plan::expense_constants(db)?,
    })
}

impl Snapshot {
    pub fn load(db: &Db, today: NaiveDate, generated_at: DateTime<Local>) -> Result<Snapshot> {
        // The canonical dates, never `App::adhoc`. The scrub is a hypothetical
        // the owner left a cursor on, it persists nothing, and a report that
        // inherited it would quote a day nobody asked about.
        let dates = projection::dates(db, today)?;
        let accounts = account::list(db)?;
        let periods_per_year =
            crate::db::setting::get_or(db, crate::db::setting::key::PAY_PERIODS_PER_YEAR, 26)?;
        // `Reading::Tolerant`, for the reason the plan section below catches
        // `transfer::plan`'s error rather than propagating it: a page cannot
        // decline to draw itself, and a report is the copy read on a phone
        // with nothing to fix the database from.
        let all = savings::rows(
            goal_engine::all_with_balances(db, Reading::Tolerant)?,
            &accounts,
            today,
            periods_per_year,
        )?;

        let containers = savings::containers_with_excess(db)?
            .into_iter()
            .map(|(id, excess)| Container {
                account: Account::named(&accounts, id),
                rows: all
                    .iter()
                    .filter(|r| r.container.id() == id)
                    .cloned()
                    .collect(),
                excess,
            })
            .collect();

        Ok(Snapshot {
            generated_at,
            overview: Overview::load(db, dates)?,
            cash: ledger(db, &accounts, Kind::Cash, today)?,
            credit: ledger(db, &accounts, Kind::Credit, today)?,
            containers,
            planning: match plan_view(db, today, dates.adhoc) {
                Ok(view) => Planning::Resolved(Box::new(view)),
                Err(e) => Planning::Unresolvable(format!("{e:#}")),
            },
            allocation: allocation_view(db, today, &accounts)?,
        })
    }
}

/// The one name the report is ever written under. A phone bookmark that
/// changed would be a bookmark nobody could keep.
pub const FILE_NAME: &str = "Money.html";

/// Written beside the report and renamed onto it, never uploaded: a partial
/// file under the real name is what this exists to prevent.
///
/// The pid is in the name because two writers can share a directory -- an
/// `mm report --dir` run overlapping the quit path of an open app -- and one
/// fixed name would have both writing the same bytes over each other before
/// either got as far as its rename.
fn temp_name() -> String {
    format!(".{FILE_NAME}.{}.tmp", std::process::id())
}

/// The page as it goes to the disk, rather than as `html::page` writes it.
///
/// The whole file travels to a phone through a sync folder and is read there
/// offline, so the bytes are worth shrinking; `html::page` stays readable
/// because that is what its own tests assert against, and this is the one
/// place between it and the disk that both writers pass through.
///
/// `minify_css` because the page's entire layout is one inline `<style>`.
/// Not `minify_js`: the page carries no script by rule -- that is what
/// `the_page_makes_no_external_request_and_carries_no_script` holds up -- so
/// a JS minifier here would only ever run over nothing.
fn minify(page: &str) -> Vec<u8> {
    let cfg = Cfg {
        minify_css: true,
        ..Cfg::new()
    };
    minify_html::minify(page.as_bytes(), &cfg)
}

/// A page that reached the disk: where it landed, and how big it is.
#[derive(Debug)]
pub struct Written {
    pub path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug)]
pub enum Outcome {
    /// No `[report]` section.
    Disabled,
    /// A `--demo` run.
    Skipped,
    /// A page is already there, written on the day this run quotes, over a
    /// database this run did not touch. See [`is_due`].
    Unchanged,
    Written(Written),
}

/// Whether the page is owed a rewrite: this run changed something, or the
/// page on the disk is not this day's.
///
/// The shape [`crate::backup::is_due`] has, and for the reason that one has
/// it -- the decision is arithmetic over three values, so it is answerable
/// without a filesystem or a clock.
///
/// `last_written` is the day the existing page was written, as its mtime, and
/// `None` covers every reason there is no such day: no page there, or a
/// directory that will not answer. Both are due, because a page that is not
/// there cannot be the one this run would have written.
///
/// The two halves are the two things a page depends on that this crate can
/// see. `wrote_rows` is the database, through [`crate::db::Db::wrote_rows`]
/// -- and only as far back as this run, which is what leaves an out-of-band
/// change able to strand a page until the next run writes a row. The day is
/// the rest: every figure on the page is quoted at a date derived from
/// `today`, and the footer's stamp is the freshness a reader checks on the
/// way out, so a page from yesterday is rewritten even when every figure on
/// it would come out the same.
///
/// **The mtime is the day the page was written, not the day it quotes**, and
/// the two part company whenever the run that wrote it was not quoting its own
/// wall-clock day. Two runs are not. `today` is read once in `main`, before
/// the screens open, so a session held across local midnight quotes the day it
/// started and lands its page on the next one -- and every quit that day then
/// finds an mtime matching and leaves a page quoting yesterday standing. A
/// `--today` run quotes whatever it was told, so the simulated page it writes
/// carries the real day's mtime and suppresses the next ordinary quit in the
/// same way. Both need the quoted day recorded somewhere the next run can read
/// it, which is a second file beside `backup.toml` and deliberately not part
/// of this gate. What is left is bounded by the other half: the next run that
/// writes a row rewrites the page whatever its date says.
pub fn is_due(last_written: Option<NaiveDate>, today: NaiveDate, wrote_rows: bool) -> bool {
    wrote_rows || last_written != Some(today)
}

/// The local day `path` was last written, or `None` for every reason it
/// cannot be read -- absent, unreadable, or a platform with no mtime. All of
/// those mean the same thing to [`is_due`], which is that this run cannot
/// claim the page already on the disk is its own.
fn written_on(path: &Path) -> Option<NaiveDate> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(DateTime::<Local>::from(modified).date_naive())
}

/// The two steps that can fail with a temporary file on the disk, so the one
/// caller has a single error path to clean up after.
fn write_then_rename(temp: &Path, path: &Path, page: &[u8]) -> Result<()> {
    std::fs::write(temp, page).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(temp, path).with_context(|| format!("renaming onto {}", path.display()))
}

/// Write the report into `dir`, whatever the config says.
///
/// The half `mm report` calls: being asked for is what the `[report]` section
/// is for the quit path, so this one has no "off" to return. Every caller
/// reaches the disk through here, which is what keeps the atomic rename from
/// having a second, sloppier implementation the day a second caller appears.
pub fn write(db: &Db, dir: &Path, today: NaiveDate) -> Result<Written> {
    let snapshot = Snapshot::load(db, today, Local::now())?;
    let page = minify(&html::page(&snapshot));

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // The temp file sits in the same directory so the rename stays atomic
    // rather than crossing a filesystem, and a sync client never sees the
    // report half-written.
    let temp = dir.join(temp_name());
    let path = dir.join(FILE_NAME);
    // A rename that cannot happen -- a read-only directory, a sync client
    // holding the target open -- would otherwise leave a partial page in the
    // synced folder under a name nothing ever cleans up.
    write_then_rename(&temp, &path, &page).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })?;

    Ok(Written {
        path,
        bytes: page.len() as u64,
    })
}

/// Write the report on quit, if this run is one that should.
///
/// `demo` skips before anything is queried. `crate::demo::install` sets a
/// thread-local flag that is still set when `main` regains control, so a
/// report written under it would be a page of scrambled figures overwriting
/// the one file that cannot be regenerated without quitting an ordinary
/// session.
///
/// The skip is the whole of the guard. The page's *figures* are formatted
/// off `Cents` directly rather than through `demo::figure`, but its account
/// names and transaction descriptions go through `account_label::Account` and
/// `description::render`, which mask where the text becomes a display and are
/// shared with the screens. So a page written with the mask installed would
/// come out as real figures beside pseudonymous names -- worse than either --
/// and there is no second line of defence to fall back on.
pub fn write_if_enabled(
    db: &Db,
    cfg: &crate::config::Config,
    today: NaiveDate,
    demo: bool,
) -> Result<Outcome> {
    let Some(report) = cfg.report.as_ref() else {
        return Ok(Outcome::Disabled);
    };
    if demo {
        return Ok(Outcome::Skipped);
    }
    let dir = report.dir()?;
    // The page this run would write is the page already there, so the rename
    // is not worth making: the directory is a synced one, and a rename onto
    // the name is what a sync client uploads and a phone downloads again.
    if !is_due(written_on(&dir.join(FILE_NAME)), today, db.wrote_rows()) {
        return Ok(Outcome::Unchanged);
    }
    Ok(Outcome::Written(write(db, &dir, today)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;
    use std::path::Path;

    fn scratch(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mistermanager_report_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn configured(dir: &Path) -> config::Config {
        toml::from_str(&format!(
            "[report]\ndir = {:?}\n",
            dir.display().to_string()
        ))
        .unwrap()
    }

    fn seeded() -> Db {
        let db = crate::db::open_in_memory().unwrap();
        account::insert(&db, "CHK", "Everyday", account::Kind::Cash, 0, None).unwrap();
        db
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 21).unwrap()
    }

    /// What reaches the disk is minified, and the minifier lowercases the
    /// doctype -- so the case `html::page` spells it in says nothing about
    /// the file, and a test that asserted on it would be reading the source
    /// rather than the page.
    fn is_the_page(text: &str) -> bool {
        text.to_ascii_lowercase().starts_with("<!doctype html>")
    }

    /// Two investment accounts holding something, one holding nothing, and a
    /// fund nobody has fetched a filing for. Every figure invented; see
    /// `CLAUDE.md`.
    fn with_holdings() -> Db {
        use crate::db::fund_mix::{AssetClass, Slice};
        let db = seeded();
        let insert = |code: &str, name: &str| {
            account::insert(
                &db,
                code,
                name,
                Kind::Investment,
                0,
                Some(account::TaxTreatment::Taxable),
            )
            .unwrap()
        };
        let brokerage = insert("BRK", "Holdings");
        let retirement = insert("RET", "Long Haul");
        insert("HSA", "Health Pot");

        holding::insert(&db, brokerage, "USM", Cents::from_dollars(6_000)).unwrap();
        holding::insert(&db, brokerage, "UNC", Cents::from_dollars(2_000)).unwrap();
        holding::insert(&db, retirement, "USB", Cents::from_dollars(2_000)).unwrap();
        let filed = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();
        let whole = |class| {
            vec![Slice {
                class,
                weight: crate::rate::BasisPoints::ONE,
            }]
        };
        let named = |t| Some(crate::test_support::fund_name(t));
        fund_mix::set_for_ticker(&db, "USM", filed, named("USM"), &whole(AssetClass::UsStock))
            .unwrap();
        fund_mix::set_for_ticker(&db, "USB", filed, named("USB"), &whole(AssetClass::UsBond))
            .unwrap();
        db
    }

    /// The portfolio is apportioned over the same holdings the sections are,
    /// so the two cannot round to different compositions -- and an account
    /// holding nothing is left out rather than drawn as a heading over an
    /// empty table.
    #[test]
    fn the_look_through_is_read_per_account_and_over_the_whole_portfolio() {
        use crate::allocation::weight;
        use crate::db::fund_mix::AssetClass;
        use crate::rate::BasisPoints;

        let db = with_holdings();
        let accounts = account::list(&db).unwrap();
        let view = allocation_view(&db, today(), &accounts).unwrap();

        // $6,000 of stock and $2,000 of bonds priced, out of $10,000 held.
        assert_eq!(
            weight(&view.lookthrough.slices, AssetClass::UsStock),
            BasisPoints(7_500)
        );
        assert_eq!(
            weight(&view.lookthrough.slices, AssetClass::UsBond),
            BasisPoints(2_500)
        );
        assert_eq!(
            view.lookthrough.coverage().as_deref(),
            Some("2 of 3 holdings")
        );

        let sections: Vec<String> = view
            .accounts
            .iter()
            .map(|a| a.account.render_with(|text, _| text.to_string()))
            .collect();
        assert_eq!(
            sections,
            ["Holdings", "Long Haul"],
            "an account holding nothing drew a section"
        );
        // The bond account holds one fund and all of it is bonds, where the
        // portfolio above is a quarter bonds.
        assert_eq!(
            weight(&view.accounts[1].lookthrough.slices, AssetClass::UsBond),
            BasisPoints::ONE
        );
        assert_eq!(view.accounts[1].lookthrough.coverage(), None);
        assert_eq!(view.accounts[0].holdings.len(), 2);
        assert!(
            view.accounts[0].holdings.iter().any(|h| h.as_of.is_none()),
            "the unfetched fund came back with a filing date"
        );
    }

    /// A summary with no composition behind it is no summary at all, rather
    /// than three zeroes measured against the age rule's targets.
    #[test]
    fn a_portfolio_with_no_filing_on_record_carries_no_summary_rows() {
        let db = seeded();
        let account = account::insert(
            &db,
            "BRK",
            "Holdings",
            Kind::Investment,
            0,
            Some(account::TaxTreatment::Taxable),
        )
        .unwrap();
        holding::insert(&db, account, "UNC", Cents::from_dollars(1_000)).unwrap();
        let view = allocation_view(&db, today(), &account::list(&db).unwrap()).unwrap();

        assert!(view.summary.is_empty(), "a summary over nothing");
        assert_eq!(view.accounts.len(), 1, "the holdings went missing with it");
        assert!(view.accounts[0].summary.is_empty());
    }

    /// The grouping leans on `txn::list` returning rows in date order, so a
    /// month that came back split in two would mean the query stopped
    /// sorting -- and the page would then carry the same month twice, each
    /// half under its own dropdown entry.
    #[test]
    fn a_ledger_groups_its_rows_into_one_entry_per_month_oldest_first() {
        let db = seeded();
        let account = account::list(&db).unwrap()[0].id;
        let write = |date: NaiveDate, description: &str| {
            crate::db::txn::insert(
                &db,
                &crate::db::txn::NewTxn {
                    date,
                    cents: Cents::from_dollars(10),
                    account_id: account,
                    description: description.to_string(),
                    recurring_txn_id: None,
                },
            )
            .unwrap();
        };
        let day = |m, d| NaiveDate::from_ymd_opt(2026, m, d).unwrap();
        // Out of date order, and July straddling August, so the grouping is
        // doing the work rather than the insert order.
        write(day(8, 3), "second");
        write(day(7, 14), "first");
        write(day(8, 28), "third");

        let ledger = ledger(&db, &account::list(&db).unwrap(), Kind::Cash, today()).unwrap();

        let months: Vec<&str> = ledger.months.iter().map(|m| m.key.as_str()).collect();
        assert_eq!(months, ["2026-07", "2026-08"], "months out of order");
        assert_eq!(ledger.months[0].label, "Jul 2026");
        let august: Vec<&str> = ledger.months[1]
            .rows
            .iter()
            .map(|r| r.description.as_str())
            .collect();
        assert_eq!(august, ["second", "third"], "rows out of order");
        // `today` is the 21st, so the 28th has not happened yet.
        assert!(
            !ledger.months[1].rows[0].future,
            "a past row read as future"
        );
        assert!(ledger.months[1].rows[1].future, "a future row read as past");
    }

    /// A failure reading the plug's asks costs the annotation and nothing
    /// else: the transfers still resolve, and every figure in the waterfall
    /// below them is still right, exactly as `App::planning_view` keeps them
    /// on the screen. Propagated instead, it would replace the whole tab with
    /// one sentence over a gap the tab could simply omit.
    ///
    /// Reachable because `transfer::plan` reaches the strict target reader
    /// only through the plug, and skips a line at zero: a payday whose fixed
    /// bills took the whole excess has a plug of nothing, so `plan` returns
    /// `Ok` where the asks beside it do not.
    #[test]
    fn a_failure_reading_the_plugs_asks_costs_the_annotation_and_nothing_else() {
        let db = seeded();
        let account = account::list(&db).unwrap()[0].id;
        account::set_group(&db, account, account::Group::Checking).unwrap();
        crate::db::txn::insert(
            &db,
            &crate::db::txn::NewTxn {
                // $100 past the default target and buffer, so the excess
                // is a hundred dollars and the housing cap takes all of it.
                date: today(),
                cents: Cents::from_dollars(15_100),
                account_id: account,
                description: "opening".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        // A housing bill far past that, so the cap leaves the plug nothing
        // and `Line::CurrentHousing` is the only line with anything in it.
        crate::db::bill::insert(
            &db,
            &crate::db::bill::NewBill {
                label: "Mortgage".to_string(),
                cents: Cents::from_dollars(10_000),
                category: crate::db::bill::Category::Housing,
                sort: 0,
            },
        )
        .unwrap();
        let container =
            account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 1, None).unwrap();
        crate::db::goal::insert(
            &db,
            &crate::db::goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: container,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
                floating: false,
                note: None,
            },
        )
        .unwrap();

        let view = plan_view(&db, today(), today()).unwrap();

        assert_eq!(
            view.plan.lines.goals,
            Cents::ZERO,
            "the plug moved something"
        );
        // The transfers are unaffected: the housing line is real, resolves,
        // and is what the owner opens the tab to read.
        let transfers = view.transfers.as_ref().expect("the transfers failed");
        assert_eq!(transfers.len(), 1);
        // Only the gap those asks would have measured is missing, and an
        // ask of nothing draws no row.
        assert_eq!(view.spread_ask_total, Cents::ZERO);
        assert_eq!(
            crate::transfer::unmet_asks(view.plan.lines.goals, view.spread_ask_total),
            None
        );
        // And the waterfall the failure says nothing about is intact.
        assert!(view.plan.shortfall.total() > Cents::ZERO);
        assert_eq!(view.housing.len(), 1);
    }

    /// The payday `Unmet Asks` exists for is the one where every line is
    /// zero, and that is the payday `transfer::plan` refuses outright -- so
    /// the asks the tab measures the plug against cannot be read through that
    /// call. Asserted on `plan_view` and not on a fixture: a `PlanView` built
    /// by hand carries whatever total it is given, and `html::planning` draws
    /// the row off that, so the production read is what this pins.
    #[test]
    fn a_payday_with_nothing_to_transfer_still_reports_what_its_goals_asked() {
        let db = seeded();
        let account = account::list(&db).unwrap()[0].id;
        account::set_group(&db, account, account::Group::Checking).unwrap();
        // An excess of nothing puts every line at zero -- what a payday whose
        // fixed bills took the whole of it produces, and what `plan` refuses.
        crate::db::setting::set(&db, crate::db::setting::key::PINNED_EXCESS, Cents::ZERO).unwrap();
        let container =
            account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 1, None).unwrap();
        crate::db::goal::insert(
            &db,
            &crate::db::goal::NewGoal {
                name: "Couch".to_string(),
                container_account_id: container,
                base_cents: Cents::from_dollars(1_000),
                // One pay period out, so it asks for the whole of what it
                // lacks rather than a fraction of it.
                goal_date: Some(today() + chrono::Duration::days(14)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
                floating: false,
                note: None,
            },
        )
        .unwrap();

        let view = plan_view(&db, today(), today()).unwrap();

        assert_eq!(
            view.plan.lines.goals,
            Cents::ZERO,
            "the plug moved something"
        );
        let Err(message) = &view.transfers else {
            panic!("a payday with every line at zero resolved to transfers");
        };
        assert_eq!(message, transfer::NOTHING_TO_TRANSFER);
        // And the asks are still read, so the tab still says the plug covers
        // none of them.
        assert_eq!(view.spread_ask_total, Cents::from_dollars(1_000));
        assert_eq!(
            transfer::unmet_asks(view.plan.lines.goals, view.spread_ask_total),
            Some(Cents::from_dollars(-1_000))
        );
    }

    /// An unset feature is an off feature -- the rule the `setting` keys and
    /// the `[backup]` section already follow.
    #[test]
    fn no_report_section_writes_nothing() {
        let outcome =
            write_if_enabled(&seeded(), &config::Config::default(), today(), false).unwrap();
        assert!(matches!(outcome, Outcome::Disabled));
    }

    /// The mask is process-global and still installed when `main` regains
    /// control, so a report written under it would be a page of scrambled
    /// figures over the one file that cannot be regenerated without quitting
    /// a normal session.
    #[test]
    fn a_demo_run_writes_nothing_and_leaves_an_existing_report_untouched() {
        let dir = scratch("demo");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "the real figures").unwrap();

        let outcome = write_if_enabled(&seeded(), &configured(&dir), today(), true).unwrap();

        assert!(matches!(outcome, Outcome::Skipped));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "the real figures");
    }

    /// A page that is not there yet is always owed, whatever the run did.
    #[test]
    fn a_directory_with_no_page_in_it_is_due() {
        assert!(is_due(None, today(), false));
    }

    /// The case this gate exists for: a second quit on the same day over a
    /// database nothing touched would rename an identical page onto the one
    /// already there, and a sync client would upload it.
    #[test]
    fn todays_page_over_an_untouched_database_is_not_due() {
        assert!(!is_due(Some(today()), today(), false));
    }

    #[test]
    fn a_run_that_wrote_a_row_is_due_however_fresh_the_page_is() {
        assert!(is_due(Some(today()), today(), true));
    }

    /// The stamp in the footer is what a reader checks on the way out, so a
    /// page carrying yesterday's date is rewritten even though every figure
    /// on it would come out the same.
    #[test]
    fn yesterdays_page_is_due_though_nothing_changed() {
        let yesterday = today().pred_opt().unwrap();
        assert!(is_due(Some(yesterday), today(), false));
    }

    /// The whole gate, over a real database rather than three arguments: a
    /// second quit finds the page it would have written already on the disk
    /// and leaves it exactly where it is, mtime included -- a rename is what
    /// a sync client uploads.
    #[test]
    fn a_second_quit_over_an_unchanged_database_leaves_the_page_untouched() {
        let dir = scratch("unchanged");
        let db_dir = scratch("unchanged-db");
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("money.db");
        // The real day, not this module's `today()`: what the gate compares
        // against is the day the file on the disk was written, and that is a
        // fact about the clock rather than about the financial date a run is
        // quoting. A directory with no page in it is due whatever day it is
        // told, so the first write does not care which one this is.
        {
            let db = crate::db::open(&db_path).unwrap();
            account::insert(&db, "CHK", "Everyday", account::Kind::Cash, 0, None).unwrap();
            write_if_enabled(&db, &configured(&dir), Local::now().date_naive(), false).unwrap();
        }
        let before = std::fs::metadata(dir.join(FILE_NAME))
            .unwrap()
            .modified()
            .unwrap();
        // Read back off the page rather than taken from the clock again: a run
        // that crossed local midnight between the write above and here would
        // otherwise be due, and this test would go red once a year.
        let day = written_on(&dir.join(FILE_NAME)).unwrap();

        // A fresh connection over a database already at this version, with
        // nothing written through it -- the shape of a quit that changed
        // nothing.
        let db = crate::db::open(&db_path).unwrap();
        let outcome = write_if_enabled(&db, &configured(&dir), day, false).unwrap();

        assert!(matches!(outcome, Outcome::Unchanged), "{outcome:?}");
        let after = std::fs::metadata(dir.join(FILE_NAME))
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(before, after, "the page was rewritten");
    }

    /// `default_db` creates its own directory; a feature that silently does
    /// nothing is the failure nothing downstream notices.
    #[test]
    fn a_missing_report_directory_is_created() {
        let dir = scratch("missing");
        let outcome = write_if_enabled(&seeded(), &configured(&dir), today(), false).unwrap();
        assert!(matches!(outcome, Outcome::Written { .. }));
        assert!(dir.join(FILE_NAME).exists());
    }

    /// A sync client watching the directory will happily upload a half-written
    /// page, so the write is a rename onto the name rather than a write to it.
    #[test]
    fn an_existing_report_is_overwritten_and_no_temporary_file_survives() {
        let dir = scratch("overwrite");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(FILE_NAME), "stale").unwrap();

        write_if_enabled(&seeded(), &configured(&dir), today(), false).unwrap();

        let page = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(is_the_page(&page), "the stale page survived");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n != FILE_NAME)
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    /// The half-written page must not outlive the failure either: a directory
    /// something is syncing is exactly where a stray `.Money.html.<pid>.tmp`
    /// would be uploaded and then sit forever, since nothing ever looks for
    /// one by that name again.
    #[test]
    fn a_rename_that_cannot_happen_leaves_no_temporary_file_behind() {
        let dir = scratch("failed-rename");
        // A directory under the report's own name: the rename onto it fails,
        // where the write of the temporary file beside it succeeds.
        std::fs::create_dir_all(dir.join(FILE_NAME)).unwrap();

        write(&seeded(), &dir, today()).expect_err("renaming onto a directory should fail");

        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n != FILE_NAME)
            .collect();
        assert!(leftovers.is_empty(), "left behind {leftovers:?}");
    }

    /// A `Cfg` that quietly did nothing would leave the page exactly as
    /// `html::page` wrote it, and nothing else on the way to the disk would
    /// notice.
    #[test]
    fn the_page_reaches_the_disk_smaller_than_it_was_written() {
        let dir = scratch("minified");
        let db = seeded();
        let source = html::page(&Snapshot::load(&db, today(), Local::now()).unwrap());

        let written = write(&db, &dir, today()).unwrap();

        let page = std::fs::read_to_string(&written.path).unwrap();
        assert!(
            page.len() < source.len(),
            "the page was not minified: {} bytes in, {} out",
            source.len(),
            page.len()
        );
        assert_eq!(
            written.bytes,
            page.len() as u64,
            "the reported size is not the size that landed"
        );
    }

    /// The bar is a row of empty `<span>`s whose whole content is an inline
    /// width, which is the one shape on the page an aggressive minifier
    /// could take for nothing at all -- and a blank track would draw as a
    /// portfolio holding none of anything. No other test would see it:
    /// `html`'s own run before any of this.
    #[test]
    fn minification_leaves_the_allocation_bar_its_widths() {
        let dir = scratch("bar");
        let written = write(&with_holdings(), &dir, today()).unwrap();
        let page = std::fs::read_to_string(&written.path).unwrap();
        // Quotes come off ahead of the match rather than into it, so which
        // attributes the minifier unquotes stays its business.
        // `75.00%` as `html` writes it: the CSS minifier reaches inside a
        // `style` attribute too, and drops a trailing zero the same as it
        // would in the stylesheet.
        let unquoted = page.replace('"', "");
        assert!(
            unquoted.contains("width:75%"),
            "the U.S. stock segment lost its width: {page}"
        );
        assert!(
            page.contains("div.bar{"),
            "the bar lost the rule that gives it a track"
        );
    }

    /// Every control on the page is a radio and a sibling selector, so a
    /// minifier that dropped an `id`, unquoted one the CSS matches on, or
    /// reordered an input past the panel it shows would leave a page that
    /// renders and then does nothing -- and no other test would see it,
    /// because `html`'s own tests run before any of this.
    #[test]
    fn minification_leaves_every_tab_and_its_switch_intact() {
        let dir = scratch("switches");
        let written = write(&seeded(), &dir, today()).unwrap();
        let page = std::fs::read_to_string(&written.path).unwrap();

        assert!(is_the_page(&page), "the doctype did not survive");
        // Both halves of a control are matched by something only the element
        // carries, because the CSS names each of them too: `Cash` and
        // `Credit` are Overview band labels as well as tab labels, and
        // `{id}-panel` is a substring of the very selector below. So the
        // label is matched paired with its `for`, and the panel by its
        // `id=`, which no selector spells. Quotes come off ahead of the
        // match rather than into it, so which attributes the minifier
        // unquotes stays its business.
        let unquoted = page.replace('"', "");
        for (id, name) in html::TABS {
            assert!(
                unquoted.contains(&format!("for={id}>{name}</label>")),
                "no {id} tab label"
            );
            assert!(
                unquoted.contains(&format!("id={id}-panel")),
                "no {id} panel"
            );
            assert!(
                page.contains(&format!("#{id}:checked~#{id}-panel")),
                "nothing switches the {id} panel on"
            );
        }
    }

    #[test]
    fn a_written_report_reports_the_path_it_landed_at() {
        let dir = scratch("path");
        let outcome = write_if_enabled(&seeded(), &configured(&dir), today(), false).unwrap();
        match outcome {
            Outcome::Written(w) => {
                assert_eq!(w.path, dir.join(FILE_NAME));
                assert!(w.bytes > 0);
            }
            other => panic!("expected a written report, got {other:?}"),
        }
    }

    /// `mm report` was asked for the page, so the section that switches the
    /// quit-time write on has no say: an owner who wants one copy, once,
    /// should not have to configure a standing report to get it.
    #[test]
    fn write_needs_no_report_section_because_being_asked_is_the_switch() {
        let dir = scratch("explicit");
        let written = write(&seeded(), &dir, today()).unwrap();
        assert_eq!(written.path, dir.join(FILE_NAME));
        assert!(is_the_page(
            &std::fs::read_to_string(&written.path).unwrap()
        ));
    }
}
