//! The standing HTML report: what the screens show, as one page a phone can
//! open -- the Overview, both ledgers, Savings, Planning and Funds, each a tab
//! of it.
//!
//! `html` writes the page readably and `write` minifies it on the way to the
//! disk, so what a test asserts against and what a phone downloads are the
//! same page in two forms.

pub mod html;

use crate::account_label::Account;
use crate::calc;
use crate::calc::planning::PlanSettings;
use crate::db::account::Kind;
use crate::db::{Db, account, bill, txn};
use crate::fund;
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
use std::path::{Path, PathBuf};

/// One container's goals and its unallocated remainder.
pub struct Container {
    pub account: Account,
    pub rows: Vec<savings::Row>,
    pub excess: Cents,
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
    pub funds: fund::Allocation,
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

/// The waterfall, its bills, and the transfers it would write.
///
/// `adhoc` and not `App::adhoc`: `Excess (Actual)` *is* the checking balance
/// at that date, so the figure here has to be quoted at the same day the
/// Overview's Paycheck-Eve column is.
fn plan_view(db: &Db, today: NaiveDate, adhoc: NaiveDate, period_days: i64) -> Result<PlanView> {
    let settings = plan::settings_from_db(db)?;
    let plan = plan::compute_from_db(db, adhoc)?;
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
    let spread_ask_total = transfer::spread_asks(db, today, period_days)
        .map(|asks| asks.iter().map(|(_, ask)| *ask).sum())
        .unwrap_or(Cents::ZERO);
    let transfers = match transfer::plan(db, &plan.lines) {
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
    })
}

impl Snapshot {
    pub fn load(db: &Db, today: NaiveDate, generated_at: DateTime<Local>) -> Result<Snapshot> {
        // The canonical dates, never `App::adhoc`. The scrub is a hypothetical
        // the owner left a cursor on, it persists nothing, and a report that
        // inherited it would quote a day nobody asked about.
        let dates = projection::dates(db, today)?;
        let accounts = account::list(db)?;
        let period_days =
            crate::db::setting::get_or(db, crate::db::setting::key::PAY_PERIOD_DAYS, 14)?;
        // `Reading::Tolerant`, for the reason the plan section below catches
        // `transfer::plan`'s error rather than propagating it: a page cannot
        // decline to draw itself, and a report is the copy read on a phone
        // with nothing to fix the database from.
        let all = savings::rows(
            goal_engine::all_with_balances(db, Reading::Tolerant)?,
            &accounts,
            today,
            period_days,
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
            planning: match plan_view(db, today, dates.adhoc, period_days) {
                Ok(view) => Planning::Resolved(Box::new(view)),
                Err(e) => Planning::Unresolvable(format!("{e:#}")),
            },
            funds: fund::compute_from_db(db, today)?,
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
    Written(Written),
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
/// report written under it would be a page of blocks overwriting the one file
/// that cannot be regenerated without quitting an ordinary session.
///
/// The page itself never calls `demo::figure`: it formats `Cents` directly.
/// Belt and braces on the skip above.
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
    Ok(Outcome::Written(write(db, &report.dir()?, today)?))
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
        account::insert(&db, "CHK", "Everyday", account::Kind::Cash, 0).unwrap();
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
        let container = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 1).unwrap();
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
            },
        )
        .unwrap();

        let view = plan_view(&db, today(), today(), 14).unwrap();

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
        let container = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 1).unwrap();
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
            },
        )
        .unwrap();

        let view = plan_view(&db, today(), today(), 14).unwrap();

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
    /// control, so a report written under it would be a page of blocks over
    /// the one file that cannot be regenerated without quitting a normal
    /// session.
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
