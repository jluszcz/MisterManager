use super::cell::{self, as_cents, as_date, as_text};
use super::{SheetRange, Sheets, sheet};
use crate::db::account::{self, Account};
use crate::db::goal::{self, NewGoal};
use crate::db::recurring_goal::{self, Cadence};
use crate::db::setting::Key;
use crate::db::{Db, GoalId, setting};
use crate::gate::Gate;
use crate::money::Cents;
use crate::plan_line::Line;
use crate::savings_block::Block;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};

#[derive(Debug, Default)]
pub struct Imported {
    pub goals: usize,
    pub buckets: usize,
    pub recurring_goals: usize,
}

/// Which account each block of the sheet belongs to.
///
/// Resolved once, before anything is written, so that a `--replace` -- which
/// clears `setting` -- cannot take the mapping out from under the read that
/// needs it. See [`containers`].
#[derive(Debug)]
pub struct Containers {
    pub goals: Account,
    pub buckets: Account,
}

impl Containers {
    fn of(&self, block: Block) -> &Account {
        match block {
            Block::Goals => &self.goals,
            Block::Buckets => &self.buckets,
        }
    }
}

/// The two container accounts, or `None` while the mapping is unconfigured.
///
/// The three states are deliberately distinct, and the middle one is why this
/// returns an `Option` rather than erroring:
///
/// - **Unset** is not a failure. The sheet names its blocks by position and
///   carries no account code, so a database that has only ever seen
///   `Constants` cannot know the mapping. `import_all` reads that as "stop
///   after the accounts and ask", which is what makes the first import
///   against an empty database a two-step.
/// - **Set and resolving** is the ordinary state.
/// - **Set and dangling** is a corrupt database and a loud error naming the
///   key, per the root `CLAUDE.md`: a key pointing at a row that is gone must
///   never be quietly reinterpreted as "not configured", which here would
///   silently re-open the two-step and then import a whole sheet into a
///   container the owner never chose.
///
/// Half-configured -- one key set and the other not -- reads as unset, since
/// there is no half of a `Savings` import to run.
pub fn containers(db: &Db) -> Result<Option<Containers>> {
    let mut found = Vec::with_capacity(Block::ALL.len());
    for block in Block::ALL {
        let key = block.key();
        let Some(id) = setting::get(db, key)? else {
            return Ok(None);
        };
        found.push(
            account::get(db, id)
                .with_context(|| format!("setting {key} names the {} block", block.label()))?,
        );
    }
    let mut found = found.into_iter();
    Ok(Some(Containers {
        goals: found.next().expect("Block::ALL names the goal block"),
        buckets: found.next().expect("Block::ALL names the bucket block"),
    }))
}

/// Write the mapping back, after a `--replace` has cleared `setting`.
///
/// The accounts it names outlive the clear -- `account` is not an imported
/// table -- so the mapping is still true, and losing it would turn every
/// `--replace` back into a two-step import.
pub fn set_containers(db: &Db, containers: &Containers) -> Result<()> {
    for block in Block::ALL {
        setting::set(db, block.key(), containers.of(block).id)?;
    }
    Ok(())
}

/// `Planning!J7` forces this bucket's interest weight to zero.
///
/// Matched loosely with `contains`, not equality: a rename in the workbook
/// (this bucket has been renamed before) must not silently make it
/// interest-eligible and misallocate every future interest posting.
///
/// Deliberately holds the same text as `Line::FutureHousing`'s substring
/// rather than deriving from it. The two answer different questions -- one is
/// `Planning!J7`'s interest weight, the other is which bucket the future
/// housing line watches -- and collapsing them would couple the day one of
/// them needs to change.
const INTEREST_INELIGIBLE_SUBSTRING: &str = "Down Payment";

/// One goal, matched by name here and recorded as a setting for the readers
/// to resolve back by id.
///
/// Name matching happens at import, once, and never at read time: several
/// names repeat in the live goal block -- one appears three times -- so
/// nothing downstream may key a goal by name.
#[derive(Debug)]
struct GoalMatch {
    key: Key<GoalId>,
    substring: &'static str,
    found: Option<(GoalId, String)>,
}

impl GoalMatch {
    fn new(key: Key<GoalId>, substring: &'static str) -> Self {
        GoalMatch {
            key,
            substring,
            found: None,
        }
    }

    fn gate(gate: Gate) -> Self {
        GoalMatch::new(gate.key(), gate.substring())
    }

    /// Offer a freshly inserted goal to this key.
    ///
    /// A second match is refused rather than resolved by a rule like "first
    /// wins": choosing wrongly would send a whole Planning line's money to
    /// the wrong goal, silently, which is the failure this indirection
    /// removes.
    fn offer(&mut self, id: GoalId, name: &str) -> Result<()> {
        if !name.contains(self.substring) {
            return Ok(());
        }
        if let Some((first_id, first_name)) = &self.found {
            bail!(
                "{} is ambiguous: goals {first_id} {first_name:?} and {id} {name:?} both match {:?}",
                self.key,
                self.substring
            );
        }
        self.found = Some((id, name.to_string()));
        Ok(())
    }

    /// No match writes nothing, which downstream means the destination is
    /// not configured.
    fn record(&self, db: &Db) -> Result<()> {
        if let Some((id, _)) = &self.found {
            setting::set(db, self.key, *id)?;
        }
        Ok(())
    }
}

/// A raw bucket row, scanned but not yet inserted.
#[derive(Debug)]
struct BucketRow {
    name: String,
    current: Cents,
    target: Cents,
}

/// Scans `Savings!I:K` from row 6, stopping at the first row with a blank
/// name in column I.
///
/// Unlike the goal block, the bucket block is contiguous, so scanning the
/// whole sheet the way the goal loop does would import any stray text further
/// down column I as a phantom bucket carrying a real allocation, silently
/// corrupting the container reconciliation. Stopping at the first blank still
/// allows the block to grow another bucket.
///
/// That same contiguity is what makes a named row with a blank `J` or `K` a
/// hard error: the scan has already ended at the first blank name, so such a
/// row is a bucket with a missing figure rather than a heading or a footer.
/// Skipping it would drop the bucket's balance from the container's
/// allocations and leave the unallocated remainder wrong by that much -- the
/// way half a bill and half a fund are errors rather than skips.
fn bucket_rows(range: &SheetRange) -> Result<Vec<BucketRow>> {
    let at = |row: usize, col: usize| cell::at(range, row, col);
    let mut rows = Vec::new();
    for row in 5..range.height() {
        let Some(name) = as_text(&at(row, 8)) else {
            break;
        };
        let current = as_cents(&at(row, 9)).with_context(|| {
            format!(
                "Savings row {} is half a bucket: {name:?} has no current value",
                row + 1
            )
        })?;
        let target = as_cents(&at(row, 10)).with_context(|| {
            format!(
                "Savings row {} is half a bucket: {name:?} has no goal",
                row + 1
            )
        })?;
        rows.push(BucketRow {
            name,
            current,
            target,
        });
    }
    Ok(rows)
}

/// The two blocks, into the two containers [`containers`] resolved.
///
/// The containers are passed in rather than read here, so that the one read
/// happens before a `--replace` clears `setting` -- and so that this cannot
/// be called at all against a database that has not been configured.
///
/// Nothing here writes `account.interest_policy`. How a container divides an
/// interest posting is a judgment about that account rather than a fact the
/// sheet carries, so it is typed on the Accounts screen and left alone by
/// every import after the row's first insert.
pub fn import(
    db: &Db,
    sheets: &mut Sheets,
    today: NaiveDate,
    containers: &Containers,
) -> Result<Imported> {
    let range = sheet(sheets, "Savings")?;
    let at = |row: usize, col: usize| cell::at(&range, row, col);

    let batch = goal::insert_batch(db, goal::BatchKind::Import, today)?;
    let mut report = Imported::default();

    // Each key is offered goals from one block only -- Roth, Bill Payments
    // and Housing from the goal block; Emergency Fund, Down Payment and
    // Mom & Dad from the bucket block -- so a future goal named "Emergency
    // Dental" in the wrong block cannot hijack a line's destination.
    let mut roth = GoalMatch::gate(Gate::Roth);
    let mut emergency = GoalMatch::gate(Gate::EmergencyFund);
    let (fh_key, fh_substring) = Line::FutureHousing
        .owned_goal()
        .expect("Future Housing is matched by name");
    let mut future_housing = GoalMatch::new(fh_key, fh_substring);
    let matched = |line: Line| -> GoalMatch {
        let (key, substring) = line
            .owned_goal()
            .expect("this line is matched by name at import");
        GoalMatch::new(key, substring)
    };
    let mut bills = matched(Line::Bills);
    let mut current_housing = matched(Line::CurrentHousing);
    let mut mom_and_dad = matched(Line::MomAndDad);

    // --- The goal block: A=name, B=current, C=goal, E=goal date, row 6 on ---
    for row in 5..range.height() {
        let Some(name) = as_text(&at(row, 0)) else {
            continue;
        };
        let Some(current) = as_cents(&at(row, 1)) else {
            continue;
        };
        let Some(target) = as_cents(&at(row, 2)) else {
            continue;
        };
        let id = goal::insert(
            db,
            &NewGoal {
                name: name.clone(),
                container_account_id: containers.goals.id,
                base_cents: target,
                goal_date: as_date(&at(row, 4)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: report.goals as i64,
                // The sheet's goal column holds whatever the owner typed, tax
                // included where they applied it, and carries no flag beside
                // it. An imported goal therefore arrives holding its target,
                // which is what untaxed means -- and holding one at all,
                // which is what not floating means.
                taxed: false,
                floating: false,
            },
        )?;
        roth.offer(id, &name)?;
        bills.offer(id, &name)?;
        current_housing.offer(id, &name)?;
        goal::insert_allocation(
            db,
            id,
            today,
            current,
            Some("imported balance"),
            Some(batch),
        )?;
        report.goals += 1;
    }

    // --- The bucket block: I=name, J=current, K=goal, row 6 on ---
    for b in bucket_rows(&range)? {
        let eligible = !b.name.contains(INTEREST_INELIGIBLE_SUBSTRING);
        let id = goal::insert(
            db,
            &NewGoal {
                name: b.name.clone(),
                container_account_id: containers.buckets.id,
                base_cents: b.target,
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: eligible,
                sort: report.buckets as i64,
                taxed: false,
                floating: false,
            },
        )?;
        emergency.offer(id, &b.name)?;
        future_housing.offer(id, &b.name)?;
        mom_and_dad.offer(id, &b.name)?;
        goal::insert_allocation(
            db,
            id,
            today,
            b.current,
            Some("imported balance"),
            Some(batch),
        )?;
        report.buckets += 1;
    }

    roth.record(db)?;
    emergency.record(db)?;
    future_housing.record(db)?;
    bills.record(db)?;
    current_housing.record(db)?;
    mom_and_dad.record(db)?;

    // --- Recurring Goals: O=name, P=date, Q=amount, cadence from the header
    // rows ---
    let mut cadence: Option<Cadence> = None;
    for row in 0..range.height() {
        let label = as_text(&at(row, 14));
        if let Some(label) = &label {
            if label.starts_with("Annual Goals") {
                cadence = Some(Cadence::Annual);
                continue;
            }
            if label.starts_with("Biannual Goals") {
                // The workbook's "biannual" means every two years.
                cadence = Some(Cadence::Biennial);
                continue;
            }
        }
        let (Some(name), Some(cadence)) = (label, cadence) else {
            continue;
        };
        let (Some(date), Some(amount)) = (as_date(&at(row, 15)), as_cents(&at(row, 16))) else {
            continue;
        };
        recurring_goal::insert(
            db,
            &recurring_goal::NewEntry {
                name,
                month: date.month() as i64,
                base_cents: amount,
                taxed: false,
                cadence,
            },
        )?;
        report.recurring_goals += 1;
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::{Cell, Data, Range};

    fn cell(row: u32, col: u32, value: Data) -> Cell<Data> {
        Cell::new((row, col), value)
    }

    /// The bucket block is contiguous, unlike the goal block (which has a
    /// gap in it). `bucket_rows` must stop at the first blank name in column
    /// I, even when stray text sits further down that column -- otherwise
    /// that text would import as a phantom bucket carrying a real
    /// allocation, corrupting the container reconciliation. The live
    /// workbook does not exhibit that, so it is covered here against a
    /// hand-built range rather than only against the real file.
    #[test]
    fn stops_scanning_at_the_first_blank_name_in_column_i() {
        // A cell at (0, 0) anchors the range's start at A1, matching how
        // calamine reads a real worksheet whose used range begins there.
        let range: SheetRange = Range::from_sparse(vec![
            cell(0, 0, Data::Empty),
            cell(5, 8, Data::String("Alpha".into())),
            cell(5, 9, Data::Float(100.0)),
            cell(5, 10, Data::Float(200.0)),
            cell(6, 8, Data::String("Beta".into())),
            cell(6, 9, Data::Float(50.0)),
            cell(6, 10, Data::Float(75.0)),
            // Row 7: no name -- the block ends here.
            // Stray text further down column I must never be reached.
            cell(10, 8, Data::String("stray note".into())),
            cell(10, 9, Data::Float(999.0)),
            cell(10, 10, Data::Float(999.0)),
        ]);

        let rows = bucket_rows(&range).unwrap();
        let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Alpha", "Beta"]);
        assert_eq!(rows[0].current, Cents::from_dollars(100));
        assert_eq!(rows[0].target, Cents::from_dollars(200));
        assert_eq!(rows[1].current, Cents::from_dollars(50));
        assert_eq!(rows[1].target, Cents::from_dollars(75));
    }

    /// Inside the contiguous block, a row with a name but a blank `J` or `K`
    /// is unambiguously a bucket with a missing figure -- the loop has
    /// already stopped at the first blank name, so it cannot be a heading or
    /// a footer. Dropping it would leave its balance out of the container's
    /// allocations and make the unallocated remainder wrong by that much, so
    /// half a row is an error rather than a skip -- the way half a bill and
    /// half a fund are.
    #[test]
    fn a_bucket_row_missing_an_amount_is_an_error() {
        let range: SheetRange = Range::from_sparse(vec![
            cell(0, 0, Data::Empty),
            cell(5, 8, Data::String("Alpha".into())),
            cell(5, 9, Data::Float(100.0)),
            cell(5, 10, Data::Float(200.0)),
            cell(6, 8, Data::String("Beta".into())),
            // Row 7 has a name and a target, but no current value.
            cell(6, 10, Data::Float(75.0)),
        ]);

        let err = bucket_rows(&range).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("Savings row 7"), "{text}");
        assert!(text.contains("Beta"), "{text}");
    }

    /// Two goals matching one gate's substring has no safe resolution. A rule
    /// like "first wins" would silently gate the entire waterfall on whichever
    /// goal happened to sort earlier in the sheet.
    #[test]
    fn a_second_match_makes_the_gate_ambiguous() {
        let mut gate = GoalMatch::gate(Gate::Roth);
        gate.offer(GoalId(1), "Roth IRA").unwrap();

        let err = gate.offer(GoalId(2), "Roth 401k Rollover").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("planning.goal.roth_id"), "{text}");
        assert!(text.contains("Roth IRA"), "{text}");
        assert!(text.contains("Roth 401k Rollover"), "{text}");
    }

    /// No match leaves the gate unrecorded, which downstream means "off".
    #[test]
    fn a_gate_with_no_matching_goal_stays_unrecorded() {
        let mut gate = GoalMatch::gate(Gate::Roth);
        gate.offer(GoalId(1), "Vacation 2027").unwrap();
        gate.offer(GoalId(2), "Christmas Gifts").unwrap();
        assert!(gate.found.is_none());
    }

    /// Matching is by substring, not equality: the live sheet names these
    /// "Roth IRA", "Emergency Savings", and "Home Down Payment".
    #[test]
    fn a_gate_matches_on_a_substring_of_the_goal_name() {
        let mut gate = GoalMatch::gate(Gate::EmergencyFund);
        gate.offer(GoalId(7), "Emergency Savings").unwrap();
        assert_eq!(
            gate.found,
            Some((GoalId(7), "Emergency Savings".to_string()))
        );
    }

    /// Each line matched at import is offered goals from one block only. An
    /// goal-block goal must not be able to claim a bucket-block line's key: the two
    /// blocks are different containers, and a line pointed at the wrong one
    /// transfers real money to the wrong bank.
    #[test]
    fn a_line_matches_a_goal_by_a_substring_of_its_name() {
        let (key, substring) = Line::Bills.owned_goal().unwrap();
        let mut m = GoalMatch::new(key, substring);
        m.offer(GoalId(3), "Vacation 2027").unwrap();
        m.offer(GoalId(4), "Bill Payments").unwrap();
        assert_eq!(m.found, Some((GoalId(4), "Bill Payments".to_string())));
    }

    /// The sheet names its blocks by position, so a database that has only
    /// ever seen `Constants` cannot know the mapping. That is not a failure:
    /// it is what makes the first import a two-step.
    #[test]
    fn unconfigured_containers_read_as_unset_rather_than_as_an_error() {
        let db = crate::db::open_in_memory().unwrap();
        assert!(containers(&db).unwrap().is_none());
    }

    /// Half configured is still unconfigured: there is no half of a `Savings`
    /// import to run.
    #[test]
    fn one_container_alone_reads_as_unset() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        setting::set(&db, Block::Goals.key(), id).unwrap();
        assert!(containers(&db).unwrap().is_none());
    }

    /// A key naming a row that is gone is a corrupt database, and must never
    /// be reinterpreted as "not configured" -- which would silently re-open
    /// the two-step and then import a whole sheet into a container the owner
    /// never chose. The error names the key and the block.
    #[test]
    fn a_container_setting_pointing_at_a_missing_account_is_an_error() {
        let db = crate::db::open_in_memory().unwrap();
        let id = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        setting::set(&db, Block::Goals.key(), id).unwrap();
        setting::set(&db, Block::Buckets.key(), crate::db::AccountId(999)).unwrap();

        let err = containers(&db).unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains(Block::Buckets.key().name()), "{text}");
        assert!(text.contains("999"), "{text}");
    }

    /// Both keys set and resolving is the ordinary state, and each block gets
    /// the account its own key names -- a transposition here would send a
    /// whole sheet's rows to the wrong container.
    #[test]
    fn configured_containers_resolve_to_the_accounts_their_keys_name() {
        let db = crate::db::open_in_memory().unwrap();
        let goals = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        let buckets = account::insert(&db, "BKR", "Brokerage", account::Kind::Cash, 1).unwrap();
        setting::set(&db, Block::Goals.key(), goals).unwrap();
        setting::set(&db, Block::Buckets.key(), buckets).unwrap();

        let found = containers(&db).unwrap().unwrap();
        assert_eq!(found.goals.id, goals);
        assert_eq!(found.buckets.id, buckets);
    }

    /// The mapping is written back after a `--replace` has cleared `setting`,
    /// so only the first import against an empty database is ever two steps.
    #[test]
    fn set_containers_puts_the_mapping_back() {
        let db = crate::db::open_in_memory().unwrap();
        let goals = account::insert(&db, "SAV", "Rainy Day", account::Kind::Cash, 0).unwrap();
        let buckets = account::insert(&db, "BKR", "Brokerage", account::Kind::Cash, 1).unwrap();
        setting::set(&db, Block::Goals.key(), goals).unwrap();
        setting::set(&db, Block::Buckets.key(), buckets).unwrap();
        let held = containers(&db).unwrap().unwrap();

        for block in Block::ALL {
            setting::clear(&db, block.key()).unwrap();
        }
        assert!(containers(&db).unwrap().is_none());

        set_containers(&db, &held).unwrap();

        let found = containers(&db).unwrap().unwrap();
        assert_eq!(found.goals.id, goals);
        assert_eq!(found.buckets.id, buckets);
    }

    /// Two goals matching one line's substring has no safe resolution, the
    /// same as for a gate.
    #[test]
    fn a_second_match_makes_a_lines_goal_ambiguous() {
        let (key, substring) = Line::MomAndDad.owned_goal().unwrap();
        let mut m = GoalMatch::new(key, substring);
        m.offer(GoalId(1), "Mom & Dad").unwrap();

        let err = m.offer(GoalId(2), "Mom & Dad Travel").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("planning.goal.mom_and_dad_id"), "{text}");
        assert!(text.contains("Mom & Dad Travel"), "{text}");
    }
}
