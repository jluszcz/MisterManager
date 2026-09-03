use super::date::{self, iso};
use super::{AccountId, AllocationId, BatchId, Db, GoalId, RecurringGoalId};
use crate::db::txn;
use crate::money::Cents;
use anyhow::{Context, Result, bail, ensure};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, Row, params};
use std::str::FromStr;

/// What produced a group of allocations.
///
/// The variants are exactly the schema's `CHECK (kind IN (...))` list: keep the
/// two in step, or an insert that type-checks will fail against the constraint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum BatchKind {
    /// The payday allocation worksheet.
    Paycheck,
    /// A monthly interest posting spread across a container's goals.
    Interest,
    /// A one-off allocation entered by hand.
    Adhoc,
    /// The opening balances written by `import`.
    Import,
}

impl BatchKind {
    /// Every kind, for callers that must cover them all.
    pub const ALL: [BatchKind; 4] = [
        BatchKind::Paycheck,
        BatchKind::Interest,
        BatchKind::Adhoc,
        BatchKind::Import,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BatchKind::Paycheck => "paycheck",
            BatchKind::Interest => "interest",
            BatchKind::Adhoc => "adhoc",
            BatchKind::Import => "import",
        }
    }
}

impl FromStr for BatchKind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "paycheck" => Ok(BatchKind::Paycheck),
            "interest" => Ok(BatchKind::Interest),
            "adhoc" => Ok(BatchKind::Adhoc),
            "import" => Ok(BatchKind::Import),
            other => bail!("unknown batch kind {other:?}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NewGoal {
    pub name: String,
    pub container_account_id: AccountId,
    pub base_cents: Cents,
    pub goal_date: Option<NaiveDate>,
    pub recurring_goal_id: Option<RecurringGoalId>,
    pub interest_eligible: bool,
    pub sort: i64,
    pub taxed: bool,
    pub floating: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Goal {
    pub id: GoalId,
    pub name: String,
    pub container_account_id: AccountId,
    pub base_cents: Cents,
    pub goal_date: Option<NaiveDate>,
    pub recurring_goal_id: Option<RecurringGoalId>,
    pub interest_eligible: bool,
    pub closed: bool,
    pub sort: i64,
    /// The owner's own mark on a goal, drawn as a band on the Savings screen.
    /// Not a fact the workbook carries and not one the import ever writes:
    /// a goal arrives unfavorited and `set_favorite` is the only way out of
    /// that.
    pub favorite: bool,
    /// Whether this goal's target is `calc::tax` of its base rather than the
    /// base itself. The base is what the table holds; `crate::goal::target`
    /// is the one place the derivation happens.
    pub taxed: bool,
    /// Whether this goal's target is whatever it currently holds. A goal the
    /// owner never means to finish is at its target by definition, so the
    /// base and `taxed` beside it say nothing while this is set --
    /// `crate::goal::target` reads it first and stops there.
    pub floating: bool,
}

#[derive(Clone, Debug)]
pub struct GoalWithBalance {
    pub goal: Goal,
    pub current: Cents,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Goal> {
    Ok(Goal {
        id: row.get(0)?,
        name: row.get(1)?,
        container_account_id: row.get(2)?,
        base_cents: Cents(row.get(3)?),
        goal_date: date::parse_opt(row.get(4)?, 4)?,
        recurring_goal_id: row.get(5)?,
        interest_eligible: row.get::<_, i64>(6)? != 0,
        closed: row.get::<_, i64>(7)? != 0,
        sort: row.get(8)?,
        favorite: row.get::<_, i64>(9)? != 0,
        taxed: row.get::<_, i64>(10)? != 0,
        floating: row.get::<_, i64>(11)? != 0,
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. See [`crate::db`] for the idiom.
///
/// The `with_balance` arm is those same columns qualified with the `g` alias
/// a join needs, followed by the goal's allocation sum as column index 12 --
/// what [`GoalWithBalance`] reads. Two lists rather than one, but adjacent,
/// so a column added to the table is one edit in one place.
macro_rules! select_goal {
    ($tail:literal) => {
        concat!(
            "SELECT id, name, container_account_id, base_cents, goal_date,
                    recurring_goal_id, interest_eligible, closed, sort, favorite,
                    taxed, floating
               FROM goal ",
            $tail
        )
    };
    (with_balance $tail:literal) => {
        concat!(
            "SELECT g.id, g.name, g.container_account_id, g.base_cents, g.goal_date,
                    g.recurring_goal_id, g.interest_eligible, g.closed, g.sort, g.favorite,
                    g.taxed, g.floating,
                    COALESCE((SELECT SUM(a.cents) FROM allocation a WHERE a.goal_id = g.id), 0)
               FROM goal g ",
            $tail
        )
    };
}

pub fn insert(db: &Db, goal: &NewGoal) -> Result<GoalId> {
    db.conn.execute(
        "INSERT INTO goal
           (name, container_account_id, base_cents, goal_date, recurring_goal_id, interest_eligible, sort, taxed, floating)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            goal.name,
            goal.container_account_id,
            goal.base_cents.0,
            goal.goal_date.map(iso),
            goal.recurring_goal_id,
            goal.interest_eligible as i64,
            goal.sort,
            goal.taxed as i64,
            goal.floating as i64
        ],
    )?;
    Ok(GoalId(db.conn.last_insert_rowid()))
}

/// The `sort` a new goal in this container should take: after every existing
/// one, so a reseed appends rather than interleaving.
pub fn next_sort(db: &Db, container: AccountId) -> Result<i64> {
    let next: i64 = db.conn.query_row(
        "SELECT COALESCE(MAX(sort), -1) + 1 FROM goal WHERE container_account_id = ?1",
        params![container],
        |r| r.get(0),
    )?;
    Ok(next)
}

/// Moves an undated goal to `position` among its container's undated goals,
/// and renumbers `sort` over all of them so the column stays `0..n-1`.
///
/// A position rather than a raw `sort`, for the reason `account::reorder`
/// takes one: `sort` is read through an `ORDER BY` that breaks ties by id, so
/// "put it third" has a result the caller can predict and "set sort to 2" has
/// one that depends on rows the caller never saw.
///
/// Only the undated block, because only the undated block is ordered by hand.
/// A dated goal takes its place from its date and keeps whatever `sort` it
/// was carrying, which is what it falls back on if the date is ever cleared.
/// Asking to place one is refused rather than ignored: a caller that has
/// misread what the manual order is for should hear so, not watch a move
/// quietly not happen.
///
/// A position past the end lands last rather than erroring, as a drag past
/// the bottom of a list does.
pub fn reorder(db: &Db, id: GoalId, position: usize) -> Result<()> {
    db.transaction(|db| {
        let goal = get(db, id)?.with_context(|| format!("no goal with id {id}"))?;
        ensure!(
            goal.goal_date.is_none(),
            "{} has a goal date, so its place comes from that date rather than by hand",
            // Reached by pressing a move key on a dated goal on Savings, and
            // put on the status line verbatim -- prose a viewer reads, masked
            // for the reason `move_value`'s three refusals are.
            crate::demo::text(&goal.name)
        );
        let mut undated: Vec<Goal> = list(db, goal.container_account_id)?
            .into_iter()
            .filter(|g| g.goal_date.is_none())
            .collect();
        let from = undated
            .iter()
            .position(|g| g.id == id)
            .context("the goal was just read as open and undated, so its container lists it")?;
        let moved = undated.remove(from);
        undated.insert(position.min(undated.len()), moved);
        for (sort, goal) in undated.iter().enumerate() {
            db.conn.execute(
                "UPDATE goal SET sort = ?2 WHERE id = ?1",
                params![goal.id, sort as i64],
            )?;
        }
        Ok(())
    })
}

/// Create several goals at once, all or nothing -- the annual reseed creates
/// dozens of them at once, and a partial one would leave the list
/// half-populated with nothing to say which half.
///
/// **Must be called at top level, not inside another [`Db::transaction`].**
pub fn insert_all(db: &Db, goals: &[NewGoal]) -> Result<Vec<GoalId>> {
    db.transaction(|db| goals.iter().map(|g| insert(db, g)).collect())
}

/// Whether a recurring goal entry already has a goal dated in `year`, closed
/// ones included -- a round that has been through and been closed out has
/// still been through. It is what tells a `Biennial` entry whether the year
/// ahead is its turn or the one it skips.
///
/// The bounds are the year's own ends rather than a `LIKE` or a `strftime`:
/// ISO dates compare as text, so a half-open range reads the same index an
/// equality would.
pub fn has_goal_dated_in_year(
    db: &Db,
    recurring_goal_id: RecurringGoalId,
    year: i32,
) -> Result<bool> {
    let start = NaiveDate::from_ymd_opt(year, 1, 1)
        .with_context(|| format!("{year} is not a year the calendar has"))?;
    let end = NaiveDate::from_ymd_opt(year + 1, 1, 1)
        .with_context(|| format!("the year after {year} runs off the calendar"))?;
    let found: bool = db.conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM goal
            WHERE recurring_goal_id = ?1 AND goal_date >= ?2 AND goal_date < ?3
         )",
        params![recurring_goal_id, iso(start), iso(end)],
        |r| r.get(0),
    )?;
    Ok(found)
}

/// One goal by id, `None` when there is none.
///
/// **The one row-by-id read that hands its caller an `Option`.**
/// [`super::account::get`] is the rule everywhere else -- a dangling foreign
/// key is a corrupt database, so it errors -- and a goal id is usually one
/// too, which is why [`crate::goal::shortfall`] and [`move_value`] put the `Option` back
/// into that shape immediately.
///
/// What keeps the `Option` here is that absence is *also* an ordinary answer
/// for two callers, and neither may fail: `transfer::wiring` has to draw a
/// dangling setting key as `Landing::Dangling` rather than refuse to render,
/// and the destination picker opens on a line's current goal even after it
/// has been closed. Both distinguish "gone" from "corrupt" themselves, which
/// only a caller holding the surrounding context can do.
pub fn get(db: &Db, id: GoalId) -> Result<Option<Goal>> {
    let found = db
        .conn
        .query_row(select_goal!("WHERE id = ?1"), params![id], from_row)
        .optional()?;
    Ok(found)
}

/// One container's open goals, in the order every screen shows them:
/// undated first as one block ordered by hand, then the dated ones soonest
/// first.
///
/// Two blocks rather than one order, because the two halves are arranged by
/// different things. A goal with a deadline has its place decided for it, and
/// the nearest deadline heads that block. A goal with none is the owner's to
/// arrange, which is what `sort` records and `reorder` writes;
/// among dated goals `sort` is only a tiebreak between two falling on the
/// same day, so an arrangement made in the undated block cannot reach in and
/// reorder a deadline.
pub fn list(db: &Db, container_account_id: AccountId) -> Result<Vec<Goal>> {
    let mut stmt = db.conn.prepare(select_goal!(
        "WHERE container_account_id = ?1 AND closed = 0
          ORDER BY goal_date IS NULL DESC, goal_date, sort, id"
    ))?;
    let rows = stmt.query_map(params![container_account_id], from_row)?;
    super::collect_rows(rows)
}

/// How many goals exist, closed ones included.
pub fn count(db: &Db) -> Result<i64> {
    let n: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM goal", [], |r| r.get(0))?;
    Ok(n)
}

pub fn balance(db: &Db, goal_id: GoalId) -> Result<Cents> {
    let sum: i64 = db.conn.query_row(
        "SELECT COALESCE(SUM(cents), 0) FROM allocation WHERE goal_id = ?1",
        params![goal_id],
        |r| r.get(0),
    )?;
    Ok(Cents(sum))
}

pub fn list_with_balances(
    db: &Db,
    container_account_id: AccountId,
) -> Result<Vec<GoalWithBalance>> {
    let mut stmt = db.conn.prepare(select_goal!(with_balance
        "WHERE g.container_account_id = ?1 AND g.closed = 0
          ORDER BY g.goal_date IS NULL DESC, g.goal_date, g.sort, g.id"
    ))?;
    let rows = stmt.query_map(params![container_account_id], |row| {
        Ok(GoalWithBalance {
            goal: from_row(row)?,
            current: Cents(row.get(12)?),
        })
    })?;
    super::collect_rows(rows)
}

/// Every open goal in every container, with its balance.
///
/// The Savings screen is one flat list with an `Acct` column, so the grouping
/// is in the `ORDER BY` rather than in the caller: `account.kind, sort, code`
/// is the order `account::list` and `containers` both use, and the screen's
/// `Tab` filter cycles the same sequence.
///
/// The account leg leads, so a container's goals stay together; below it is
/// [`list`]'s own order, undated block then dated, and for the same reasons.
///
pub fn all_with_balances(db: &Db) -> Result<Vec<GoalWithBalance>> {
    let mut stmt = db.conn.prepare(select_goal!(with_balance
        "JOIN account acct ON acct.id = g.container_account_id
          WHERE g.closed = 0
          ORDER BY acct.kind, acct.sort, acct.code,
                   g.goal_date IS NULL DESC, g.goal_date, g.sort, g.id"
    ))?;
    let rows = stmt.query_map([], |row| {
        Ok(GoalWithBalance {
            goal: from_row(row)?,
            current: Cents(row.get(12)?),
        })
    })?;
    super::collect_rows(rows)
}

pub fn insert_batch(db: &Db, kind: BatchKind, date: NaiveDate) -> Result<BatchId> {
    db.conn.execute(
        "INSERT INTO batch (kind, date) VALUES (?1, ?2)",
        params![kind.as_str(), iso(date)],
    )?;
    Ok(BatchId(db.conn.last_insert_rowid()))
}

/// **Must be called at top level, not inside another [`Db::transaction`].**
/// Not currently reachable from `import::import_all`, which wraps the whole
/// import in its own transaction -- keep it that way, or thread a shared
/// transaction through instead of calling this from inside one.
pub fn delete_batch(db: &Db, batch_id: BatchId) -> Result<()> {
    db.transaction(|db| {
        db.conn.execute(
            "DELETE FROM allocation WHERE batch_id = ?1",
            params![batch_id],
        )?;
        db.conn
            .execute("DELETE FROM batch WHERE id = ?1", params![batch_id])?;
        Ok(())
    })
}

pub fn insert_allocation(
    db: &Db,
    goal_id: GoalId,
    date: NaiveDate,
    cents: Cents,
    note: Option<&str>,
    batch_id: Option<BatchId>,
) -> Result<AllocationId> {
    db.conn.execute(
        "INSERT INTO allocation (goal_id, date, cents, note, batch_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![goal_id, iso(date), cents.0, note, batch_id],
    )?;
    Ok(AllocationId(db.conn.last_insert_rowid()))
}

/// One row of a goal's allocation history.
///
/// Nothing but the history modal reads an allocation back individually: every
/// other reader wants the sum, through [`balance`] or [`select_goal!`]'s
/// `with_balance` arm. The `batch_id` is not among the columns, because
/// nothing that draws a row has anything to say about which posting wrote it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allocation {
    pub id: AllocationId,
    pub goal_id: GoalId,
    pub date: NaiveDate,
    pub cents: Cents,
    pub note: Option<String>,
}

fn allocation_from_row(row: &Row<'_>) -> rusqlite::Result<Allocation> {
    Ok(Allocation {
        id: row.get(0)?,
        goal_id: row.get(1)?,
        date: date::parse(&row.get::<_, String>(2)?, 2)?,
        cents: Cents(row.get(3)?),
        note: row.get(4)?,
    })
}

/// A `SELECT` of the columns [`allocation_from_row`] reads, in the order it
/// reads them, with `$tail` appended. See [`crate::db`] for the idiom.
macro_rules! select_allocation {
    ($tail:literal) => {
        concat!(
            "SELECT id, goal_id, date, cents, note
               FROM allocation ",
            $tail
        )
    };
}

/// One goal's allocations, oldest first.
///
/// `ORDER BY date, id` is the ordering [`crate::db::txn::list`] already uses,
/// so a day holding several rows reads in the order they were written rather
/// than in whatever order SQLite hands them back.
pub fn allocations(db: &Db, goal_id: GoalId) -> Result<Vec<Allocation>> {
    let mut stmt = db
        .conn
        .prepare(select_allocation!("WHERE goal_id = ?1 ORDER BY date, id"))?;
    let rows = stmt.query_map(params![goal_id], allocation_from_row)?;
    super::collect_rows(rows)
}

/// The editable half of an allocation.
///
/// Its goal is not among them. A misdirected row is deleted and re-entered
/// against the right goal rather than re-pointed: within a container that
/// would be defensible, and across containers it would move a goal's value
/// with no cash moved between the accounts -- the boundary [`move_value`]
/// already refuses to cross.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllocationEdit {
    pub date: NaiveDate,
    pub cents: Cents,
    pub note: Option<String>,
}

/// Rewrite one allocation.
///
/// Errors when no row changed, the way [`update`] does: an id held by a
/// caller came off a row the history drew, so a miss is a corrupt database
/// rather than an ordinary absence.
pub fn update_allocation(db: &Db, id: AllocationId, edit: &AllocationEdit) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE allocation SET date = ?2, cents = ?3, note = ?4 WHERE id = ?1",
        params![id, iso(edit.date), edit.cents.0, edit.note],
    )?;
    ensure!(changed == 1, "no allocation with id {id}");
    Ok(())
}

/// Delete one allocation, leaving whatever batch it belonged to alone: a
/// batch is a group of rows entered together, not a row that has to keep
/// every member it opened with.
pub fn delete_allocation(db: &Db, id: AllocationId) -> Result<()> {
    let changed = db
        .conn
        .execute("DELETE FROM allocation WHERE id = ?1", params![id])?;
    ensure!(changed == 1, "no allocation with id {id}");
    Ok(())
}

/// A group of allocations entered together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub id: BatchId,
    pub kind: BatchKind,
    pub date: NaiveDate,
}

fn batch_from_row(row: &Row<'_>) -> rusqlite::Result<Batch> {
    let kind: String = row.get(1)?;
    Ok(Batch {
        id: row.get(0)?,
        kind: kind.parse().expect("schema CHECK guarantees a valid kind"),
        date: date::parse_opt(row.get(2)?, 2)?.expect("batch.date is NOT NULL"),
    })
}

/// Write a whole worksheet commit: one batch, one allocation per share, all
/// or nothing.
///
/// The batch is opened here rather than passed in, so "every commit writes one
/// `batch`" is structural instead of something each caller must remember. A
/// half-written payday would leave the container's reconciliation wrong with
/// nothing on screen to say so.
///
/// **Must be called at top level, not inside another [`Db::transaction`].**
pub fn insert_allocations(
    db: &Db,
    kind: BatchKind,
    date: NaiveDate,
    shares: &[(GoalId, Cents)],
    note: Option<&str>,
) -> Result<BatchId> {
    db.transaction(|db| {
        let batch = insert_batch(db, kind, date)?;
        for (goal_id, cents) in shares {
            insert_allocation(db, *goal_id, date, *cents, note, Some(batch))?;
        }
        Ok(batch)
    })
}

/// The container's most recent batch of one kind -- what the `manual` interest
/// prefill rescales.
///
/// A batch carries no container of its own; it belongs to whichever containers
/// its goals live in, which is why this joins through `allocation`.
pub fn last_batch(db: &Db, kind: BatchKind, container: AccountId) -> Result<Option<Batch>> {
    let found = db
        .conn
        .query_row(
            "SELECT b.id, b.kind, b.date FROM batch b
              WHERE b.kind = ?1
                AND EXISTS (SELECT 1 FROM allocation a JOIN goal g ON g.id = a.goal_id
                             WHERE a.batch_id = b.id AND g.container_account_id = ?2)
              ORDER BY b.date DESC, b.id DESC
              LIMIT 1",
            params![kind.as_str(), container],
            batch_from_row,
        )
        .optional()?;
    Ok(found)
}

/// The last batch entered, whatever it was -- what `U` undoes.
///
/// Ordered by id, not date: undo means "the last thing I did", and a payday
/// batch is routinely dated ahead of an interest batch entered after it.
///
/// **Never returns an `Import` batch.** That one holds every opening balance
/// in the database, and deleting it would empty every goal in one keystroke.
pub fn most_recent_batch(db: &Db) -> Result<Option<Batch>> {
    let found = db
        .conn
        .query_row(
            "SELECT id, kind, date FROM batch WHERE kind <> ?1 ORDER BY id DESC LIMIT 1",
            params![BatchKind::Import.as_str()],
            batch_from_row,
        )
        .optional()?;
    Ok(found)
}

/// What one batch put into each goal, one row per goal.
pub fn batch_shares(db: &Db, batch: BatchId) -> Result<Vec<(GoalId, Cents)>> {
    let mut stmt = db.conn.prepare(
        "SELECT goal_id, SUM(cents) FROM allocation
          WHERE batch_id = ?1 GROUP BY goal_id ORDER BY goal_id",
    )?;
    let rows = stmt.query_map(params![batch], |r| Ok((r.get(0)?, Cents(r.get(1)?))))?;
    super::collect_rows(rows)
}

/// The editable half of a goal. Its container and its recurring goal entry
/// are not editable: those are what the goal *is*, and changing a container
/// without moving cash would break the reconciliation.
///
/// Interest eligibility is here because it is a policy rather than a fact
/// about the goal. The importer sets the opening value from the workbook's
/// own weighting; whether a bucket keeps taking a share of a posting after
/// that is the owner's call.
///
/// The flag is here rather than being a column with one writer of its own,
/// the way `favorite` is: the goal form has a field for it, so an edit that
/// left it alone would make the field unable to take a mark back off.
#[derive(Clone, Debug)]
pub struct GoalEdit {
    pub name: String,
    pub base_cents: Cents,
    pub goal_date: Option<NaiveDate>,
    pub interest_eligible: bool,
    pub taxed: bool,
    pub floating: bool,
}

pub fn update(db: &Db, id: GoalId, edit: &GoalEdit) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE goal SET name = ?2, base_cents = ?3, goal_date = ?4, \
         interest_eligible = ?5, taxed = ?6, floating = ?7 WHERE id = ?1",
        params![
            id,
            edit.name,
            edit.base_cents.0,
            edit.goal_date.map(iso),
            edit.interest_eligible as i64,
            edit.taxed as i64,
            edit.floating as i64,
        ],
    )?;
    ensure!(changed == 1, "no goal with id {id}");
    Ok(())
}

/// Hide a goal from every listing. Its allocations stay, so
/// `container_excess` does not move.
pub fn close(db: &Db, id: GoalId) -> Result<()> {
    let changed = db
        .conn
        .execute("UPDATE goal SET closed = 1 WHERE id = ?1", params![id])?;
    ensure!(changed == 1, "no goal with id {id}");
    Ok(())
}

/// Mark or unmark a goal as one of the owner's favorites.
///
/// The column's one writer, the way `recurring_txn::set_paycheck` is its
/// flag's: the goal form has no field for this and `update` must not touch
/// it, or an edit would silently clear a mark the owner set with `f`.
pub fn set_favorite(db: &Db, id: GoalId, favorite: bool) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE goal SET favorite = ?2 WHERE id = ?1",
        params![id, favorite as i64],
    )?;
    ensure!(changed == 1, "no goal with id {id}");
    Ok(())
}

/// The goal value is moving *out* of, checked: it exists, and it is open.
///
/// Both refusals here and in [`open_destination`] land on the status line
/// verbatim, so the goal names in them are prose a viewer reads. Masked where
/// the message is built, the way `account::checking`'s ambiguity error is.
fn open_source(db: &Db, from: GoalId) -> Result<Goal> {
    let source = get(db, from)?.with_context(|| format!("no goal with id {from}"))?;
    ensure!(
        !source.closed,
        "{:?} is already closed",
        crate::demo::text(&source.name)
    );
    Ok(source)
}

/// The goal value is moving *into*, checked against where it is coming from.
///
/// One reading for both writers: an ending and a transfer differ in what they
/// move and in whether the source survives, and in nothing about where the
/// value may land.
fn open_destination(db: &Db, source: &Goal, to: GoalId) -> Result<Goal> {
    ensure!(source.id != to, "value cannot move from a goal into itself");
    let destination = get(db, to)?.with_context(|| format!("no goal with id {to}"))?;
    // A closed goal is hidden from every listing, so value moved into one
    // would be money no screen can show. `get` does not filter on `closed`,
    // so this is the check that keeps the rule true for every caller.
    ensure!(
        !destination.closed,
        "{:?} is closed; pick an open goal or return the value to unallocated",
        crate::demo::text(&destination.name)
    );
    // No cash crosses between containers here, so allowing it would break
    // both reconciliations at once. Getting money from Rainy Day to Brokerage
    // is a transfer on the Cash screen, and only then an allocation.
    ensure!(
        destination.container_account_id == source.container_account_id,
        "{:?} and {:?} are in different containers; transfer the cash first",
        crate::demo::text(&source.name),
        crate::demo::text(&destination.name)
    );
    Ok(destination)
}

/// End a goal: move its whole balance out and close it.
///
/// `to` is the destination goal, or `None` to return the value to
/// unallocated. Those are the two endings -- close out and abandon -- and
/// they differ only in whether the second allocation is written, so they
/// share one transaction and one same-container check.
///
/// The amount is the goal's whole balance, never a part of it: calling a
/// partial move a close-out would leave a goal closed with money in it, and
/// [`transfer_value`] is the write that moves a part and ends nothing. A
/// negative balance is allowed through; refusing it would leave an overspent
/// goal unclosable.
///
/// Both goals must be open. A goal ends once, and value moved into a closed
/// goal would be money hidden from every listing.
///
/// **Must be called at top level, not inside another [`Db::transaction`].**
/// Nothing calls it from inside `import::import_all`'s transaction.
pub fn move_value(db: &Db, from: GoalId, to: Option<GoalId>, date: NaiveDate) -> Result<()> {
    let source = open_source(db, from)?;
    if let Some(to) = to {
        open_destination(db, &source, to)?;
    }
    let moved = balance(db, from)?;
    // Plain text: a note is read by a person, like the ones typed by hand.
    // The quoting above is for error messages, where it disambiguates a name
    // with a trailing space or none at all. Unmasked for a sharper reason
    // than the quoting: this string is written to the row, and a pseudonym
    // reaching a write is the one thing a demo must never do.
    let note = format!("closed out of {}", source.name);
    db.transaction(|db| {
        insert_allocation(db, from, date, -moved, Some("closed out"), None)?;
        if let Some(to) = to {
            insert_allocation(db, to, date, moved, Some(&note), None)?;
        }
        close(db, from)
    })
}

/// Move part of a goal's value to another goal in the same container.
///
/// The close-out that ends nothing: both goals stay open, and what crosses is
/// an amount the owner typed rather than the whole balance. Whether the
/// source is left short is theirs to judge -- an overspent goal is a real
/// state, the same one [`move_value`] lets through.
///
/// Which way the value goes is the destination's to say, so the amount is
/// positive: a negative one is this same move written backwards, and zero
/// writes two rows that say nothing.
///
/// The pair is one [`BatchKind::Adhoc`] batch, so `U` reverses a fumbled
/// transfer whole. An ending is deliberately no batch, because an undo could
/// not reopen the goal it closed; a transfer closes nothing, so nothing here
/// stands in the way.
///
/// **Must be called at top level, not inside another [`Db::transaction`].**
pub fn transfer_value(
    db: &Db,
    from: GoalId,
    to: GoalId,
    amount: Cents,
    date: NaiveDate,
) -> Result<()> {
    ensure!(
        amount > Cents::ZERO,
        "a transfer moves a positive amount; pick the other goal to send it the other way"
    );
    let source = open_source(db, from)?;
    let destination = open_destination(db, &source, to)?;
    // Plain text, and each row naming the goal at the other end: these are
    // read by a person, like the ones typed by hand. Unmasked for the sharper
    // reason `move_value`'s note gives -- the string is written to the row,
    // and a pseudonym reaching a write is the one thing a demo must never do.
    let out = format!("moved to {}", destination.name);
    let into = format!("moved from {}", source.name);
    db.transaction(|db| {
        let batch = insert_batch(db, BatchKind::Adhoc, date)?;
        insert_allocation(db, from, date, -amount, Some(&out), Some(batch))?;
        insert_allocation(db, to, date, amount, Some(&into), Some(batch))?;
        Ok(())
    })
}

/// The container's unallocated remainder -- `Savings!B3`.
///
/// Deliberately undated: `Savings!B1` is a `SUMIF` over the cash ledger
/// with no date criterion, so future-dated rows count. One container
/// reconciles to exactly zero in the workbook; the other sits a few cents
/// out, which is what `savings::unallocated` truncates away before either
/// sink draws the line.
pub fn container_excess(db: &Db, container_account_id: AccountId) -> Result<Cents> {
    let held = txn::balance_all_time(db, container_account_id)?;
    let allocated: i64 = db.conn.query_row(
        "SELECT COALESCE(SUM(a.cents), 0)
           FROM allocation a JOIN goal g ON g.id = a.goal_id
          WHERE g.container_account_id = ?1",
        params![container_account_id],
        |r| r.get(0),
    )?;
    Ok(held - Cents(allocated))
}

/// The accounts that hold goals, in `account::list` order.
///
/// Overview reconciles each container's held cash against its allocations.
/// An account with no goals has nothing to reconcile, and running the check
/// against one would report its whole balance as unallocated excess.
pub fn containers(db: &Db) -> Result<Vec<AccountId>> {
    let mut stmt = db.conn.prepare(
        "SELECT DISTINCT g.container_account_id
           FROM goal g JOIN account a ON a.id = g.container_account_id
          ORDER BY a.kind, a.sort, a.code",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    super::collect_rows(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::db::account::{self, Kind};
    use crate::db::txn::{self, NewTxn};
    use crate::money::Cents;
    use crate::test_support::day;

    fn new_goal(name: &str, container: AccountId, goal: i64) -> NewGoal {
        NewGoal {
            name: name.to_string(),
            container_account_id: container,
            base_cents: Cents::from_dollars(goal),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            sort: 0,
            taxed: false,
            floating: false,
        }
    }

    #[test]
    fn batch_kind_as_str_and_from_str_round_trip() {
        for kind in BatchKind::ALL {
            assert_eq!(kind.as_str().parse::<BatchKind>().unwrap(), kind);
        }
    }

    #[test]
    fn from_str_rejects_an_unknown_batch_kind() {
        assert!("payckeck".parse::<BatchKind>().is_err());
    }

    /// `ALL` is written out by hand, so a kind added to the enum without
    /// being added here would drop out of the test below that inserts one
    /// batch of every variant looking for a missing `CHECK` arm.
    #[test]
    fn all_covers_every_variant() {
        // The match is exhaustive, so a fifth variant stops this compiling
        // until it is added to `ALL` and counted here too.
        for kind in BatchKind::ALL {
            match kind {
                BatchKind::Paycheck
                | BatchKind::Interest
                | BatchKind::Adhoc
                | BatchKind::Import => {}
            }
        }
        assert_eq!(BatchKind::ALL.len(), 4);
    }

    /// The enum and the schema's `CHECK (kind IN (...))` are two independent
    /// lists of the same four strings. A variant missing from the constraint
    /// type-checks and then fails at runtime, inside whatever transaction the
    /// caller is running -- for `Import`, the whole import. Insert one batch
    /// of every variant so that mismatch fails here instead.
    #[test]
    fn every_batch_kind_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        for kind in BatchKind::ALL {
            insert_batch(&db, kind, day(2026, 1, 1))
                .unwrap_or_else(|e| panic!("{kind:?} is not in the schema's CHECK list: {e}"));
        }
    }

    #[test]
    fn balance_is_the_sum_of_allocations() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Vacation 2027", savings, 15_000)).unwrap();

        assert_eq!(balance(&db, id).unwrap(), Cents::ZERO);

        insert_allocation(
            &db,
            id,
            day(2026, 1, 15),
            Cents::from_dollars(454),
            None,
            None,
        )
        .unwrap();
        insert_allocation(
            &db,
            id,
            day(2026, 1, 29),
            Cents::from_dollars(454),
            None,
            None,
        )
        .unwrap();
        // Spending against a goal is a negative allocation.
        insert_allocation(
            &db,
            id,
            day(2026, 2, 2),
            Cents::from_dollars(-100),
            None,
            None,
        )
        .unwrap();

        assert_eq!(balance(&db, id).unwrap(), Cents::from_dollars(808));
    }

    /// A container whose undated balance equals what its goals hold
    /// reconciles to zero; one that does not is off by the difference.
    #[test]
    fn container_excess_is_the_undated_balance_minus_allocated() {
        let db = db::open_in_memory().unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 0).unwrap();

        txn::insert(
            &db,
            &NewTxn {
                date: day(2026, 1, 1),
                cents: Cents(63_100_195), // 631,001.95
                account_id: brokerage,
                description: "End of Year".into(),
                recurring_txn_id: None,
            },
        )
        .unwrap();

        let dp = insert(&db, &new_goal("Home Down Payment", brokerage, 500_000)).unwrap();
        let em = insert(&db, &new_goal("Emergency Savings", brokerage, 100_000)).unwrap();
        let md = insert(&db, &new_goal("Mom & Dad", brokerage, 25_000)).unwrap();
        insert_allocation(&db, dp, day(2026, 1, 1), Cents(50_000_000), None, None).unwrap();
        insert_allocation(&db, em, day(2026, 1, 1), Cents(10_600_195), None, None).unwrap();
        insert_allocation(&db, md, day(2026, 1, 1), Cents(2_500_000), None, None).unwrap();

        assert_eq!(container_excess(&db, brokerage).unwrap(), Cents::ZERO);
    }

    /// Container excess ignores dates. A future-dated row still counts,
    /// because `Savings!B1` is an undated SUMIF.
    #[test]
    fn container_excess_counts_future_rows() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        txn::insert(
            &db,
            &NewTxn {
                date: day(2099, 1, 1),
                cents: Cents(10_000),
                account_id: savings,
                description: "future".into(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        assert_eq!(container_excess(&db, savings).unwrap(), Cents(10_000));
    }

    #[test]
    fn list_with_balances_returns_goals_in_sort_order() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut second = new_goal("Second", savings, 100);
        second.sort = 2;
        let mut first = new_goal("First", savings, 100);
        first.sort = 1;
        let second_id = insert(&db, &second).unwrap();
        insert(&db, &first).unwrap();
        insert_allocation(&db, second_id, day(2026, 1, 1), Cents(500), None, None).unwrap();

        let goals = list_with_balances(&db, savings).unwrap();
        assert_eq!(goals.len(), 2);
        assert_eq!(goals[0].goal.name, "First");
        assert_eq!(goals[0].current, Cents::ZERO);
        assert_eq!(goals[1].goal.name, "Second");
        assert_eq!(goals[1].current, Cents(500));
    }

    #[test]
    fn allocations_can_be_grouped_into_a_batch_and_removed_together() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let a = insert(&db, &new_goal("A", savings, 100)).unwrap();
        let b = insert(&db, &new_goal("B", savings, 100)).unwrap();

        let batch = insert_batch(&db, BatchKind::Paycheck, day(2026, 8, 28)).unwrap();
        insert_allocation(&db, a, day(2026, 8, 28), Cents(700), None, Some(batch)).unwrap();
        insert_allocation(&db, b, day(2026, 8, 28), Cents(6_400), None, Some(batch)).unwrap();

        assert_eq!(balance(&db, a).unwrap(), Cents(700));
        delete_batch(&db, batch).unwrap();
        assert_eq!(balance(&db, a).unwrap(), Cents::ZERO);
        assert_eq!(balance(&db, b).unwrap(), Cents::ZERO);
    }

    #[test]
    fn closed_goals_are_excluded_from_listings() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Done", savings, 100)).unwrap();
        db.conn
            .execute(
                "UPDATE goal SET closed = 1 WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        assert!(list_with_balances(&db, savings).unwrap().is_empty());
    }

    #[test]
    fn a_malformed_goal_date_is_an_error_not_a_silently_undated_goal() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Corrupt", savings, 100)).unwrap();
        db.conn
            .execute(
                "UPDATE goal SET goal_date = '08/27/2026' WHERE id = ?1",
                rusqlite::params![id],
            )
            .unwrap();
        assert!(get(&db, id).is_err());
    }

    #[test]
    fn get_round_trips_every_field() {
        let db = db::open_in_memory().unwrap();

        // Explicit, distinct ids/values everywhere so a transposed `select_goal!`
        // ordering shows up as a mismatch instead of passing coincidentally.
        // AccountId and RecurringGoalId now make that particular swap a
        // compile error, but interest_eligible, the always-false closed flag,
        // the always-false favorite flag, and taxed are all still the same
        // type, and auto-assigned rowids still collide at id 1 in every fresh
        // table. Do not "tidy" these back to defaults/insert(). `taxed: true`
        // against `favorite`'s fixed `false` is what catches a favorite/taxed
        // transposition: either bool reading the other's column flips exactly
        // one of the two assertions below.
        db.conn
            .execute(
                "INSERT INTO account (id, code, name, kind, grp)
                 VALUES (7, 'SAV', 'Rainy Day', 'cash', 'savings')",
                [],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO recurring_goal (id, name, month, base_cents, taxed, cadence)
             VALUES (3, 'Dropbox', 1, 12800, 0, 'annual')",
                [],
            )
            .unwrap();
        let goal = NewGoal {
            name: "Roof Replacement".to_string(),
            container_account_id: AccountId(7),
            base_cents: Cents(54_321),
            goal_date: Some(day(2027, 3, 5)),
            recurring_goal_id: Some(RecurringGoalId(3)),
            interest_eligible: true,
            sort: 9,
            taxed: true,
            floating: false,
        };
        let id = insert(&db, &goal).unwrap();

        let found = get(&db, id).unwrap().unwrap();
        assert_eq!(found.id, id);
        assert_eq!(found.name, "Roof Replacement");
        assert_eq!(found.container_account_id, AccountId(7));
        assert_eq!(found.base_cents, Cents(54_321));
        assert_eq!(found.goal_date, Some(day(2027, 3, 5)));
        assert_eq!(found.recurring_goal_id, Some(RecurringGoalId(3)));
        assert!(found.interest_eligible);
        assert!(!found.closed);
        assert_eq!(found.sort, 9);
        assert!(found.taxed);
        assert!(!found.favorite, "a goal arrives unfavorited");
    }

    #[test]
    fn get_returns_none_for_a_missing_id() {
        let db = db::open_in_memory().unwrap();
        assert!(get(&db, GoalId(999)).unwrap().is_none());
    }

    #[test]
    fn list_orders_by_sort_then_id_and_excludes_closed() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut second = new_goal("Second", savings, 100);
        second.sort = 2;
        let mut first = new_goal("First", savings, 100);
        first.sort = 1;
        insert(&db, &second).unwrap();
        insert(&db, &first).unwrap();
        let closed_id = insert(&db, &new_goal("Closed", savings, 100)).unwrap();
        db.conn
            .execute(
                "UPDATE goal SET closed = 1 WHERE id = ?1",
                rusqlite::params![closed_id],
            )
            .unwrap();

        let goals = list(&db, savings).unwrap();
        assert_eq!(goals.len(), 2);
        assert_eq!(goals[0].name, "First");
        assert_eq!(goals[1].name, "Second");
    }

    /// Undated goals come first as one block, ordered by hand; dated goals
    /// follow, soonest first. A goal with no date is one the owner has not put
    /// a deadline on, so it is theirs to arrange, and a deadline is what
    /// decides every other row's place.
    #[test]
    fn undated_goals_sort_before_dated_ones() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();

        let mut dated = new_goal("Dated", savings, 100);
        dated.goal_date = Some(day(2027, 3, 1));
        dated.sort = 0;
        insert(&db, &dated).unwrap();
        let mut undated = new_goal("Undated", savings, 100);
        undated.sort = 9;
        insert(&db, &undated).unwrap();

        let names: Vec<String> = list(&db, savings)
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, vec!["Undated", "Dated"]);
    }

    /// Soonest first: the goal due next heads the dated block, which is the
    /// order they come due in. `sort` is only a tiebreak between two goals
    /// falling on the same day, so the arrangement the owner set among the
    /// undated ones cannot reach in here and reorder a deadline.
    #[test]
    fn dated_goals_sort_soonest_first_whatever_their_manual_order() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();

        for (name, date, sort) in [
            ("Soonest", day(2026, 9, 1), 0),
            ("Furthest", day(2030, 1, 1), 5),
            ("Middle", day(2028, 6, 1), 2),
        ] {
            let mut goal = new_goal(name, savings, 100);
            goal.goal_date = Some(date);
            goal.sort = sort;
            insert(&db, &goal).unwrap();
        }

        let names: Vec<String> = list(&db, savings)
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, vec!["Soonest", "Middle", "Furthest"]);
    }

    /// The same order, through the query the Savings screen actually reads.
    #[test]
    fn all_with_balances_puts_each_containers_undated_goals_first() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();

        for (name, container, date) in [
            ("Rainy Dated", savings, Some(day(2027, 1, 1))),
            ("Rainy Undated", savings, None),
            ("Brokerage Dated", brokerage, Some(day(2027, 1, 1))),
            ("Brokerage Undated", brokerage, None),
        ] {
            let mut goal = new_goal(name, container, 100);
            goal.goal_date = date;
            insert(&db, &goal).unwrap();
        }

        let goals = all_with_balances(&db).unwrap();
        let names: Vec<&str> = goals.iter().map(|g| g.goal.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Rainy Undated",
                "Rainy Dated",
                "Brokerage Undated",
                "Brokerage Dated"
            ],
            "the account leg still leads, so a container's goals stay together"
        );
    }

    /// `reorder` takes a position and renumbers the container's undated
    /// block, so what the screen shows is what is stored -- the same bargain
    /// `account::reorder` makes with a kind.
    #[test]
    fn reorder_moves_an_undated_goal_and_renumbers_the_block() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut ids = Vec::new();
        for (name, sort) in [("First", 0), ("Second", 1), ("Third", 2)] {
            let mut goal = new_goal(name, savings, 100);
            goal.sort = sort;
            ids.push(insert(&db, &goal).unwrap());
        }

        reorder(&db, ids[2], 0).unwrap();

        let goals = list(&db, savings).unwrap();
        let names: Vec<&str> = goals.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, vec!["Third", "First", "Second"]);
        let sorts: Vec<i64> = goals.iter().map(|g| g.sort).collect();
        assert_eq!(sorts, vec![0, 1, 2], "renumbered 0..n-1");
    }

    /// A dated goal takes its place from its date, so it is not in the block
    /// being renumbered and its `sort` must come through untouched -- it is
    /// what the goal falls back on if the date is later cleared.
    #[test]
    fn reorder_leaves_the_containers_dated_goals_alone() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut dated = new_goal("Dated", savings, 100);
        dated.goal_date = Some(day(2027, 1, 1));
        dated.sort = 7;
        let dated_id = insert(&db, &dated).unwrap();
        let mut ids = Vec::new();
        for (name, sort) in [("First", 0), ("Second", 1)] {
            let mut goal = new_goal(name, savings, 100);
            goal.sort = sort;
            ids.push(insert(&db, &goal).unwrap());
        }

        reorder(&db, ids[1], 0).unwrap();

        assert_eq!(get(&db, dated_id).unwrap().unwrap().sort, 7);
    }

    /// Refused rather than renumbering around it: a caller asking to place a
    /// dated goal by hand has misread what the manual order is for, and
    /// silently doing nothing would look like the move landed.
    #[test]
    fn reordering_a_dated_goal_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut dated = new_goal("Dated", savings, 100);
        dated.goal_date = Some(day(2027, 1, 1));
        let dated_id = insert(&db, &dated).unwrap();

        assert!(reorder(&db, dated_id, 0).is_err());
    }

    #[test]
    fn reorder_past_the_end_lands_last() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut ids = Vec::new();
        for name in ["First", "Second", "Third"] {
            ids.push(insert(&db, &new_goal(name, savings, 100)).unwrap());
        }

        reorder(&db, ids[0], 99).unwrap();

        let names: Vec<String> = list(&db, savings)
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, vec!["Second", "Third", "First"]);
    }

    #[test]
    fn reorder_leaves_another_container_alone() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();
        let mut ids = Vec::new();
        for name in ["First", "Second"] {
            ids.push(insert(&db, &new_goal(name, savings, 100)).unwrap());
        }
        let mut other = new_goal("Elsewhere", brokerage, 100);
        other.sort = 4;
        let other_id = insert(&db, &other).unwrap();

        reorder(&db, ids[1], 0).unwrap();

        assert_eq!(get(&db, other_id).unwrap().unwrap().sort, 4);
    }

    /// `container_excess` still counts a closed goal's allocations, but
    /// `list_with_balances` hides the goal -- the two must not disagree.
    #[test]
    fn closing_a_goal_hides_it_but_does_not_change_container_excess() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        txn::insert(
            &db,
            &NewTxn {
                date: day(2026, 1, 1),
                cents: Cents(10_000),
                account_id: savings,
                description: "deposit".into(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        let id = insert(&db, &new_goal("Closing Soon", savings, 100)).unwrap();
        insert_allocation(&db, id, day(2026, 1, 1), Cents(10_000), None, None).unwrap();

        let before = container_excess(&db, savings).unwrap();
        close(&db, id).unwrap();

        assert!(list_with_balances(&db, savings).unwrap().is_empty());
        assert_eq!(container_excess(&db, savings).unwrap(), before);
        assert_eq!(before, Cents::ZERO);
    }

    /// Only accounts that actually hold goals may be reconciled. Running
    /// `container_excess` against Everyday checking would report its entire
    /// balance as unallocated, which is true and useless.
    #[test]
    fn containers_lists_only_the_accounts_holding_goals() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();
        insert(&db, &new_goal("Vacation", savings, 100_000)).unwrap();
        insert(&db, &new_goal("Lego", savings, 20_000)).unwrap();
        insert(&db, &new_goal("Emergency Savings", brokerage, 1_066_000)).unwrap();

        let found = containers(&db).unwrap();
        assert_eq!(found, vec![savings, brokerage], "Everyday holds no goals");
        assert!(!found.contains(&checking));
    }

    #[test]
    fn containers_of_a_database_with_no_goals_is_empty() {
        let db = db::open_in_memory().unwrap();
        account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        assert!(containers(&db).unwrap().is_empty());
    }

    /// The screen is one flat list with an `Acct` column, so the ordering has
    /// to come out of SQL already grouped by container -- in the same order
    /// `account::list` and `containers` use, or Overview and Savings would
    /// disagree about which container comes first.
    #[test]
    fn all_with_balances_orders_by_container_then_sort_then_id() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();

        let mut second = new_goal("Rainy Day Second", savings, 100);
        second.sort = 2;
        let mut first = new_goal("Rainy Day First", savings, 100);
        first.sort = 1;
        insert(&db, &second).unwrap();
        insert(&db, &first).unwrap();
        let emergency = insert(&db, &new_goal("Emergency Savings", brokerage, 1_066)).unwrap();
        insert_allocation(&db, emergency, day(2026, 1, 1), Cents(500), None, None).unwrap();

        let closed = insert(&db, &new_goal("Done", savings, 100)).unwrap();
        db.conn
            .execute(
                "UPDATE goal SET closed = 1 WHERE id = ?1",
                rusqlite::params![closed],
            )
            .unwrap();

        let goals = all_with_balances(&db).unwrap();
        let names: Vec<&str> = goals.iter().map(|g| g.goal.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Rainy Day First", "Rainy Day Second", "Emergency Savings"]
        );
        assert_eq!(goals[2].current, Cents(500));
    }

    #[test]
    fn all_with_balances_of_a_database_with_no_goals_is_empty() {
        let db = db::open_in_memory().unwrap();
        account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        assert!(all_with_balances(&db).unwrap().is_empty());
    }

    /// Abandoning returns the goal's value to unallocated, so the container's
    /// excess rises by exactly the balance. That is the whole difference
    /// between the two endings, and the reconciliation is what shows it.
    #[test]
    fn abandoning_a_goal_raises_the_container_excess_by_its_balance() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        txn::insert(
            &db,
            &NewTxn {
                date: day(2026, 1, 1),
                cents: Cents(100_000),
                account_id: savings,
                description: "deposit".into(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        let before = container_excess(&db, savings).unwrap();

        move_value(&db, couch, None, day(2026, 8, 16)).unwrap();

        assert_eq!(balance(&db, couch).unwrap(), Cents::ZERO);
        assert!(get(&db, couch).unwrap().unwrap().closed);
        assert_eq!(
            container_excess(&db, savings).unwrap(),
            before + Cents(60_000)
        );
    }

    /// A close-out moves value between two goals in the same container, so no
    /// cash moved and the reconciliation must not move either.
    #[test]
    fn closing_a_goal_out_into_another_leaves_the_container_excess_unmoved() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        let before = container_excess(&db, savings).unwrap();

        move_value(&db, couch, Some(rug), day(2026, 8, 16)).unwrap();

        assert_eq!(balance(&db, couch).unwrap(), Cents::ZERO);
        assert_eq!(balance(&db, rug).unwrap(), Cents(60_000));
        assert!(get(&db, couch).unwrap().unwrap().closed);
        assert!(!get(&db, rug).unwrap().unwrap().closed);
        assert_eq!(container_excess(&db, savings).unwrap(), before);
    }

    /// Moving Rainy Day value into a Brokerage bucket would break both
    /// reconciliations at once, since no cash crossed between the accounts.
    /// The form restricts the destination; this is the backstop.
    #[test]
    fn closing_a_goal_out_across_containers_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let emergency = insert(&db, &new_goal("Emergency Savings", brokerage, 1_066)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        let err = move_value(&db, couch, Some(emergency), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("different containers"), "{err}");
        assert!(
            !get(&db, couch).unwrap().unwrap().closed,
            "a refused close-out must not close the goal"
        );
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
    }

    /// An overspent goal is a real state. Refusing a negative balance would
    /// leave it unclosable, and the arithmetic is the same either way.
    #[test]
    fn a_goal_with_a_negative_balance_can_still_be_closed_out() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(-2_500), None, None).unwrap();
        insert_allocation(&db, rug, day(2026, 1, 1), Cents(10_000), None, None).unwrap();

        move_value(&db, couch, Some(rug), day(2026, 8, 16)).unwrap();

        assert_eq!(balance(&db, couch).unwrap(), Cents::ZERO);
        assert_eq!(balance(&db, rug).unwrap(), Cents(7_500));
    }

    #[test]
    fn closing_a_goal_out_into_itself_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        assert!(move_value(&db, couch, Some(couch), day(2026, 8, 16)).is_err());
    }

    /// The note is read by a person, so it holds the goal's name as plain
    /// text -- the same shape as a note typed by hand. Quoting belongs in
    /// error messages, where the delimiters disambiguate a name with an odd
    /// shape; a note is data, not diagnostics.
    #[test]
    fn a_close_out_notes_the_goal_the_value_came_from_in_plain_text() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        move_value(&db, couch, Some(rug), day(2026, 8, 16)).unwrap();

        let note: String = db
            .conn
            .query_row(
                "SELECT note FROM allocation WHERE goal_id = ?1 AND cents > 0",
                rusqlite::params![rug],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(note, "closed out of Couch");
    }

    /// A closed goal is hidden from every listing, so value moved into one
    /// would be money the screens cannot show and the owner cannot spend
    /// against. The close-out form only offers open goals; this is the
    /// backstop, so the rule holds for any future caller too.
    #[test]
    fn closing_a_goal_out_into_a_closed_goal_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        close(&db, rug).unwrap();

        let err = move_value(&db, couch, Some(rug), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
        assert!(
            !get(&db, couch).unwrap().unwrap().closed,
            "a refused close-out must not close the goal"
        );
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents::ZERO);
    }

    /// A transfer is a close-out that does not end anything: value crosses
    /// between two goals of one container, so no cash moved and the
    /// reconciliation must not move either. Both goals stay open, which is
    /// the whole difference from `move_value`.
    #[test]
    fn transferring_value_between_goals_leaves_both_open_and_the_excess_unmoved() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        let before = container_excess(&db, savings).unwrap();

        transfer_value(&db, couch, rug, Cents(25_000), day(2026, 8, 16)).unwrap();

        assert_eq!(balance(&db, couch).unwrap(), Cents(35_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents(25_000));
        assert!(!get(&db, couch).unwrap().unwrap().closed);
        assert!(!get(&db, rug).unwrap().unwrap().closed);
        assert_eq!(container_excess(&db, savings).unwrap(), before);
    }

    /// The two rows are written together, so they are undone together: `U`
    /// reaches the most recent batch, and a fumbled amount is one keystroke
    /// back rather than two hand-corrections in two histories.
    #[test]
    fn a_transfer_writes_both_rows_under_one_adhoc_batch() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        transfer_value(&db, couch, rug, Cents(25_000), day(2026, 8, 16)).unwrap();

        let batch = last_batch(&db, BatchKind::Adhoc, savings)
            .unwrap()
            .expect("the transfer opened a batch");
        assert_eq!(batch.date, day(2026, 8, 16));
        let rows: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM allocation WHERE batch_id = ?1",
                rusqlite::params![batch.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 2);

        delete_batch(&db, batch.id).unwrap();
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents::ZERO);
    }

    /// Each row names the goal at the other end of the transfer, in plain
    /// text, for the reason a close-out's note is plain: it is read by a
    /// person, and a pseudonym reaching a write is the one thing a demo must
    /// never do.
    #[test]
    fn a_transfer_notes_the_goal_at_the_other_end_of_it_in_plain_text() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();

        transfer_value(&db, couch, rug, Cents(25_000), day(2026, 8, 16)).unwrap();

        let note = |goal: GoalId| -> String {
            db.conn
                .query_row(
                    "SELECT note FROM allocation WHERE goal_id = ?1",
                    rusqlite::params![goal],
                    |r| r.get(0),
                )
                .unwrap()
        };
        assert_eq!(note(couch), "moved to Rug");
        assert_eq!(note(rug), "moved from Couch");
    }

    /// No cash crossed between the accounts, so a transfer across containers
    /// would break both reconciliations at once. The form restricts the
    /// destination; this is the backstop.
    #[test]
    fn transferring_value_across_containers_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let emergency = insert(&db, &new_goal("Emergency Savings", brokerage, 1_066)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        let err =
            transfer_value(&db, couch, emergency, Cents(25_000), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("different containers"), "{err}");
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, emergency).unwrap(), Cents::ZERO);
    }

    /// A closed goal is hidden from every listing, so value moved into one
    /// would be money no screen can show -- whichever write moved it.
    #[test]
    fn transferring_value_into_a_closed_goal_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        close(&db, rug).unwrap();

        let err = transfer_value(&db, couch, rug, Cents(25_000), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents::ZERO);
    }

    /// A closed goal is finished. Taking value back out of one would reopen
    /// the question the ending answered, and leave a closed goal holding a
    /// balance no listing shows.
    #[test]
    fn transferring_value_out_of_a_closed_goal_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();
        close(&db, couch).unwrap();

        let err = transfer_value(&db, couch, rug, Cents(25_000), day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents::ZERO);
    }

    #[test]
    fn transferring_value_from_a_goal_into_itself_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        assert!(transfer_value(&db, couch, couch, Cents(25_000), day(2026, 8, 16)).is_err());
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
    }

    /// Which way the value goes is the destination's to say, so a transfer
    /// moves a positive amount or none at all: a negative one is the same
    /// move written backwards, and zero writes two rows that say nothing.
    #[test]
    fn a_transfer_of_a_non_positive_amount_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let rug = insert(&db, &new_goal("Rug", savings, 1_000)).unwrap();
        insert_allocation(&db, couch, day(2026, 1, 1), Cents(60_000), None, None).unwrap();

        for amount in [Cents::ZERO, Cents(-25_000)] {
            let err = transfer_value(&db, couch, rug, amount, day(2026, 8, 16)).unwrap_err();
            assert!(err.to_string().contains("positive"), "{err}");
        }
        assert_eq!(balance(&db, couch).unwrap(), Cents(60_000));
        assert_eq!(balance(&db, rug).unwrap(), Cents::ZERO);
    }

    /// A goal ends once. Ending an ended goal would write a second pair of
    /// allocations against it and close it again, which is meaningless rather
    /// than harmful -- but the two endings are what this function *is*, so it
    /// refuses instead of pretending to perform one.
    #[test]
    fn ending_a_goal_that_is_already_closed_is_refused() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        close(&db, couch).unwrap();

        let err = move_value(&db, couch, None, day(2026, 8, 16)).unwrap_err();
        assert!(err.to_string().contains("closed"), "{err}");
    }

    #[test]
    fn update_rewrites_the_name_target_and_goal_date() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();

        update(
            &db,
            id,
            &GoalEdit {
                name: "Sofa".to_string(),
                base_cents: Cents(150_000),
                goal_date: Some(day(2027, 3, 5)),
                interest_eligible: true,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();

        let found = get(&db, id).unwrap().unwrap();
        assert_eq!(found.name, "Sofa");
        assert_eq!(found.base_cents, Cents(150_000));
        assert_eq!(found.goal_date, Some(day(2027, 3, 5)));
    }

    /// Clearing the date field turns a dated goal back into an undated one,
    /// which is what blanks column E in the sheet.
    #[test]
    fn update_can_clear_a_goal_date() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut goal = new_goal("Couch", savings, 1_000);
        goal.goal_date = Some(day(2027, 3, 5));
        let id = insert(&db, &goal).unwrap();

        update(
            &db,
            id,
            &GoalEdit {
                name: "Couch".to_string(),
                base_cents: Cents(100_000),
                goal_date: None,
                interest_eligible: true,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();

        assert_eq!(get(&db, id).unwrap().unwrap().goal_date, None);
    }

    /// A floating goal's target follows its balance, so the flag is the whole
    /// of what says so -- the base beside it is inert while it is set. An edit
    /// made for any other field would otherwise fix the goal to that base.
    #[test]
    fn a_goals_floating_flag_round_trips_through_insert_and_update() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let mut goal = new_goal("Brokerage", savings, 0);
        goal.floating = true;
        let id = insert(&db, &goal).unwrap();
        assert!(get(&db, id).unwrap().unwrap().floating);

        update(
            &db,
            id,
            &GoalEdit {
                name: "Brokerage".to_string(),
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                interest_eligible: true,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();

        assert!(!get(&db, id).unwrap().unwrap().floating);
    }

    /// The state every existing goal is already in, and the one the schema
    /// default writes.
    #[test]
    fn a_goal_arrives_not_floating() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();

        assert!(!get(&db, id).unwrap().unwrap().floating);
    }

    /// Eligibility is a policy the owner sets, not a fact about the goal, so
    /// the edit form carries it and `update` is where it lands.
    #[test]
    fn update_rewrites_interest_eligibility() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Down Payment", savings, 1_000)).unwrap();

        update(
            &db,
            id,
            &GoalEdit {
                name: "Down Payment".to_string(),
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                interest_eligible: false,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();

        assert!(!get(&db, id).unwrap().unwrap().interest_eligible);
    }

    /// A goal arrives unfavorited: the highlight on the Savings screen is an
    /// override the owner applies, never a state a goal is imported into.
    #[test]
    fn a_goal_is_not_favorited_until_it_is_made_one() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Down Payment", savings, 1_000)).unwrap();

        assert!(!get(&db, id).unwrap().unwrap().favorite);
    }

    /// `f` is a toggle, so both directions have to land -- a favorite that
    /// could be set and not cleared would be a decision the owner cannot take
    /// back.
    #[test]
    fn favoriting_a_goal_round_trips_and_can_be_taken_back() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Down Payment", savings, 1_000)).unwrap();

        set_favorite(&db, id, true).unwrap();
        assert!(get(&db, id).unwrap().unwrap().favorite);

        set_favorite(&db, id, false).unwrap();
        assert!(!get(&db, id).unwrap().unwrap().favorite);
    }

    /// The column has one writer, and the edit form is not it: `update`
    /// rewrites the three fields the form carries and must leave a favorite
    /// exactly where the toggle put it.
    #[test]
    fn editing_a_goal_leaves_its_favorite_alone() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let id = insert(&db, &new_goal("Down Payment", savings, 1_000)).unwrap();
        set_favorite(&db, id, true).unwrap();

        update(
            &db,
            id,
            &GoalEdit {
                name: "Home Down Payment".to_string(),
                base_cents: Cents::from_dollars(2_000),
                goal_date: None,
                interest_eligible: true,
                taxed: false,
                floating: false,
            },
        )
        .unwrap();

        assert!(get(&db, id).unwrap().unwrap().favorite);
    }

    #[test]
    fn favoriting_a_missing_goal_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(set_favorite(&db, GoalId(999), true).is_err());
    }

    #[test]
    fn updating_or_closing_a_missing_goal_is_an_error() {
        let db = db::open_in_memory().unwrap();
        assert!(close(&db, GoalId(999)).is_err());
        assert!(
            update(
                &db,
                GoalId(999),
                &GoalEdit {
                    name: "Ghost".to_string(),
                    base_cents: Cents(100),
                    goal_date: None,
                    interest_eligible: true,
                    taxed: false,
                    floating: false,
                },
            )
            .is_err()
        );
    }

    /// Every worksheet commit is one batch, so a fumbled payday is one
    /// `delete_batch` rather than dozens of deletions.
    #[test]
    fn insert_allocations_writes_one_batch_for_every_share() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let a = insert(&db, &new_goal("A", savings, 100)).unwrap();
        let b = insert(&db, &new_goal("B", savings, 100)).unwrap();

        let batch = insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 8, 28),
            &[(a, Cents(254_400)), (b, Cents(69_300))],
            Some("payday"),
        )
        .unwrap();

        assert_eq!(balance(&db, a).unwrap(), Cents(254_400));
        assert_eq!(balance(&db, b).unwrap(), Cents(69_300));
        assert_eq!(
            batch_shares(&db, batch).unwrap(),
            vec![(a, Cents(254_400)), (b, Cents(69_300))]
        );

        delete_batch(&db, batch).unwrap();
        assert_eq!(balance(&db, a).unwrap(), Cents::ZERO);
        assert_eq!(balance(&db, b).unwrap(), Cents::ZERO);
    }

    /// All or nothing: a half-written payday would leave the container's
    /// reconciliation wrong with nothing on screen to say so.
    #[test]
    fn insert_allocations_writes_nothing_when_any_row_fails() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let a = insert(&db, &new_goal("A", savings, 100)).unwrap();

        let err = insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 8, 28),
            &[(a, Cents(700)), (GoalId(999), Cents(700))],
            None,
        )
        .unwrap_err();
        assert!(!err.to_string().is_empty());

        assert_eq!(balance(&db, a).unwrap(), Cents::ZERO);
        let batches: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM batch", [], |r| r.get(0))
            .unwrap();
        assert_eq!(batches, 0, "the batch row survived a rolled-back commit");
    }

    /// The `manual` interest prefill reads the container's own last interest
    /// posting -- not a paycheck batch, and not another container's.
    #[test]
    fn last_batch_ignores_other_kinds_and_other_containers() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();
        let roth_goal = insert(&db, &new_goal("Roth IRA", savings, 100)).unwrap();
        let emergency_goal = insert(&db, &new_goal("Emergency Savings", brokerage, 100)).unwrap();

        insert_allocations(
            &db,
            BatchKind::Interest,
            day(2026, 6, 30),
            &[(roth_goal, Cents(30_000))],
            None,
        )
        .unwrap();
        let july = insert_allocations(
            &db,
            BatchKind::Interest,
            day(2026, 7, 31),
            &[(roth_goal, Cents(45_017))],
            None,
        )
        .unwrap();
        insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 8, 14),
            &[(roth_goal, Cents(70_000))],
            None,
        )
        .unwrap();
        insert_allocations(
            &db,
            BatchKind::Interest,
            day(2026, 8, 15),
            &[(emergency_goal, Cents(120_048))],
            None,
        )
        .unwrap();

        let found = last_batch(&db, BatchKind::Interest, savings)
            .unwrap()
            .unwrap();
        assert_eq!(found.id, july, "the newest Rainy Day interest batch");
        assert_eq!(found.date, day(2026, 7, 31));
        assert_eq!(
            batch_shares(&db, found.id).unwrap(),
            vec![(roth_goal, Cents(45_017))]
        );
    }

    #[test]
    fn last_batch_of_a_container_with_no_such_posting_is_none() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        assert!(
            last_batch(&db, BatchKind::Interest, savings)
                .unwrap()
                .is_none()
        );
    }

    /// `U` undoes the last thing entered, which is insert order and not date
    /// order: a payday batch is routinely dated ahead of an interest batch
    /// entered after it.
    #[test]
    fn most_recent_batch_is_the_last_one_entered() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let g = insert(&db, &new_goal("Roth IRA", savings, 100)).unwrap();
        insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 8, 28),
            &[(g, Cents(700))],
            None,
        )
        .unwrap();
        let interest = insert_allocations(
            &db,
            BatchKind::Interest,
            day(2026, 8, 15),
            &[(g, Cents(400))],
            None,
        )
        .unwrap();

        let found = most_recent_batch(&db).unwrap().unwrap();
        assert_eq!(found.id, interest);
        assert_eq!(found.kind, BatchKind::Interest);
        assert_eq!(found.date, day(2026, 8, 15));
    }

    /// The import batch holds every opening balance in the database. Offering
    /// it to `U` would empty every goal in one keystroke.
    #[test]
    fn most_recent_batch_never_offers_the_import_batch() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let g = insert(&db, &new_goal("Roth IRA", savings, 100)).unwrap();
        insert_allocations(
            &db,
            BatchKind::Import,
            day(2026, 8, 12),
            &[(g, Cents(500_000))],
            None,
        )
        .unwrap();

        assert!(most_recent_batch(&db).unwrap().is_none());

        let paycheck = insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 8, 14),
            &[(g, Cents(700))],
            None,
        )
        .unwrap();
        assert_eq!(most_recent_batch(&db).unwrap().unwrap().id, paycheck);
    }

    /// One row per goal, summed: the interest prefill wants shares, and `U`'s
    /// confirmation counts goals rather than allocation rows.
    #[test]
    fn batch_shares_sums_a_goals_rows_together() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let g = insert(&db, &new_goal("Roth IRA", savings, 100)).unwrap();
        let batch = insert_batch(&db, BatchKind::Adhoc, day(2026, 8, 14)).unwrap();
        insert_allocation(&db, g, day(2026, 8, 14), Cents(700), None, Some(batch)).unwrap();
        insert_allocation(&db, g, day(2026, 8, 14), Cents(300), None, Some(batch)).unwrap();

        assert_eq!(batch_shares(&db, batch).unwrap(), vec![(g, Cents(1_000))]);
    }

    /// A new goal sorts after the container's existing ones, so the annual
    /// reseed appends rather than interleaving with the imported order.
    #[test]
    fn next_sort_follows_the_containers_highest_sort() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 1).unwrap();
        assert_eq!(next_sort(&db, savings).unwrap(), 0);

        let mut goal = new_goal("Lego", savings, 340);
        goal.sort = 7;
        insert(&db, &goal).unwrap();
        assert_eq!(next_sort(&db, savings).unwrap(), 8);
        assert_eq!(next_sort(&db, brokerage).unwrap(), 0, "per container");
    }

    /// The annual reseed creates dozens of goals at once; a partial one would
    /// leave the list half-populated with nothing to say which half.
    #[test]
    fn insert_all_writes_every_goal_or_none_of_them() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let ids = insert_all(
            &db,
            &[
                new_goal("Lego", savings, 340),
                new_goal("Dropbox", savings, 128),
            ],
        )
        .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(list(&db, savings).unwrap().len(), 2);

        // A container that does not exist: the foreign key refuses the second
        // row, and the first must go with it.
        let orphan = new_goal("Orphan", AccountId(999), 100);
        assert!(insert_all(&db, &[new_goal("Rug", savings, 100), orphan]).is_err());
        assert_eq!(
            list(&db, savings).unwrap().len(),
            2,
            "the good row survived"
        );
    }

    /// What tells a `Biennial` entry whether it has had this year's round --
    /// closed goals included, since a round that has been through and been
    /// closed out has still been through.
    #[test]
    fn a_goal_dated_in_the_year_is_found_even_once_it_is_closed() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let entry = crate::db::recurring_goal::insert(
            &db,
            &crate::db::recurring_goal::NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: crate::db::recurring_goal::Cadence::Biennial,
            },
        )
        .unwrap();
        assert!(!has_goal_dated_in_year(&db, entry, 2026).unwrap());

        let mut goal = new_goal("Backblaze", savings, 99);
        goal.recurring_goal_id = Some(entry);
        goal.goal_date = Some(day(2026, 11, 1));
        let id = insert(&db, &goal).unwrap();
        close(&db, id).unwrap();

        assert!(has_goal_dated_in_year(&db, entry, 2026).unwrap());
    }

    /// December and January are one day apart and two years, so the bounds
    /// have to be the year's own ends rather than anything looser.
    #[test]
    fn a_goal_dated_in_a_neighbouring_year_does_not_count() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let entry = crate::db::recurring_goal::insert(
            &db,
            &crate::db::recurring_goal::NewEntry {
                name: "Backblaze".to_string(),
                month: 12,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: crate::db::recurring_goal::Cadence::Biennial,
            },
        )
        .unwrap();
        for date in [day(2025, 12, 31), day(2027, 1, 1)] {
            let mut goal = new_goal("Backblaze", savings, 99);
            goal.recurring_goal_id = Some(entry);
            goal.goal_date = Some(date);
            insert(&db, &goal).unwrap();
        }

        assert!(!has_goal_dated_in_year(&db, entry, 2026).unwrap());
        assert!(has_goal_dated_in_year(&db, entry, 2025).unwrap());
        assert!(has_goal_dated_in_year(&db, entry, 2027).unwrap());
    }

    /// An undated goal belongs to no year, the same as it belongs to no month
    /// on the Savings filter.
    #[test]
    fn an_undated_goal_counts_for_no_year() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let entry = crate::db::recurring_goal::insert(
            &db,
            &crate::db::recurring_goal::NewEntry {
                name: "Backblaze".to_string(),
                month: 11,
                base_cents: Cents::from_dollars(99),
                taxed: false,
                cadence: crate::db::recurring_goal::Cadence::Biennial,
            },
        )
        .unwrap();
        let mut goal = new_goal("Backblaze", savings, 99);
        goal.recurring_goal_id = Some(entry);
        goal.goal_date = None;
        insert(&db, &goal).unwrap();

        assert!(!has_goal_dated_in_year(&db, entry, 2026).unwrap());
    }
    /// The history reads a whole row back, which nothing else in the crate
    /// does -- every other reader wants the sum. Distinct values in every
    /// column, so a transposed `select_allocation!` fails here.
    #[test]
    fn allocations_read_back_every_column_of_a_goals_rows() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let id = insert_allocation(
            &db,
            couch,
            day(2026, 3, 4),
            Cents::from_dollars(250),
            Some("birthday money"),
            None,
        )
        .unwrap();

        assert_eq!(
            allocations(&db, couch).unwrap(),
            vec![Allocation {
                id,
                goal_id: couch,
                date: day(2026, 3, 4),
                cents: Cents::from_dollars(250),
                note: Some("birthday money".to_string()),
            }]
        );
    }

    /// A note is `NULL` where none was given, not an empty string: an
    /// allocation with nothing said about it draws an em dash rather than a
    /// blank cell, and `Option` is what carries that difference up.
    #[test]
    fn an_allocation_with_no_note_round_trips_as_none() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        insert_allocation(
            &db,
            couch,
            day(2026, 3, 4),
            Cents::from_dollars(250),
            None,
            None,
        )
        .unwrap();

        assert_eq!(allocations(&db, couch).unwrap()[0].note, None);
    }

    /// Oldest first, and two rows on one date read in the order they were
    /// written -- the tiebreak `ORDER BY date, id` exists for.
    #[test]
    fn allocations_are_oldest_first_and_break_a_shared_date_by_insert_order() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let write = |date, note: &str| {
            insert_allocation(&db, couch, date, Cents::from_dollars(100), Some(note), None).unwrap()
        };
        // Out of order, and the middle date written twice.
        write(day(2026, 5, 1), "third");
        write(day(2026, 3, 4), "first");
        write(day(2026, 3, 4), "second");

        let notes: Vec<String> = allocations(&db, couch)
            .unwrap()
            .into_iter()
            .map(|a| a.note.unwrap())
            .collect();
        assert_eq!(notes, ["first", "second", "third"]);
    }

    #[test]
    fn allocations_of_another_goal_are_not_listed() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let lego = insert(&db, &new_goal("Lego", savings, 500)).unwrap();
        insert_allocation(
            &db,
            lego,
            day(2026, 3, 4),
            Cents::from_dollars(250),
            None,
            None,
        )
        .unwrap();

        assert!(allocations(&db, couch).unwrap().is_empty());
    }

    /// The correction the modal exists for: all three columns rewritten, and
    /// the goal's balance following the figure.
    #[test]
    fn update_allocation_rewrites_the_date_amount_and_note() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let id = insert_allocation(
            &db,
            couch,
            day(2026, 3, 4),
            Cents::from_dollars(250),
            Some("typo"),
            None,
        )
        .unwrap();

        update_allocation(
            &db,
            id,
            &AllocationEdit {
                date: day(2026, 3, 11),
                cents: Cents::from_dollars(150),
                note: None,
            },
        )
        .unwrap();

        assert_eq!(
            allocations(&db, couch).unwrap(),
            vec![Allocation {
                id,
                goal_id: couch,
                date: day(2026, 3, 11),
                cents: Cents::from_dollars(150),
                note: None,
            }]
        );
        assert_eq!(balance(&db, couch).unwrap(), Cents::from_dollars(150));
    }

    /// An id held by a caller came off a row the history drew, so a miss is a
    /// corrupt database rather than an ordinary absence.
    #[test]
    fn updating_or_deleting_a_missing_allocation_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let edit = AllocationEdit {
            date: day(2026, 3, 11),
            cents: Cents::from_dollars(150),
            note: None,
        };
        assert!(update_allocation(&db, AllocationId(1), &edit).is_err());
        assert!(delete_allocation(&db, AllocationId(1)).is_err());
    }

    /// Deleting one row leaves the rest of its batch alone: a batch is a
    /// group entered together, not a group that has to stay whole.
    #[test]
    fn delete_allocation_removes_one_row_and_leaves_its_batch_intact() {
        let db = db::open_in_memory().unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let couch = insert(&db, &new_goal("Couch", savings, 1_000)).unwrap();
        let lego = insert(&db, &new_goal("Lego", savings, 500)).unwrap();
        let batch = insert_allocations(
            &db,
            BatchKind::Paycheck,
            day(2026, 3, 4),
            &[
                (couch, Cents::from_dollars(250)),
                (lego, Cents::from_dollars(100)),
            ],
            None,
        )
        .unwrap();

        let couch_row = allocations(&db, couch).unwrap()[0].id;
        delete_allocation(&db, couch_row).unwrap();

        assert!(allocations(&db, couch).unwrap().is_empty());
        assert_eq!(balance(&db, couch).unwrap(), Cents::ZERO);
        assert_eq!(
            batch_shares(&db, batch).unwrap().len(),
            1,
            "Lego's row stays"
        );
    }
}
