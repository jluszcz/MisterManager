//! The `setting` table: one row per key, values stored as `TEXT`.
//!
//! Keys are not written at call sites. Each one is a `Key<T>` constant in
//! [`key`] below, carrying both the string and the type of the value stored
//! under it, so a caller cannot mistype a key or read one at the wrong type.
//! Both failures are silent otherwise: every reader has a fallback, so a
//! mistyped key reads as "not configured" and quietly supplies a default.
//!
//! The key decides the type, at both ends:
//!
//! ```
//! use mistermanager::db::{self, setting::{self, key}};
//! use mistermanager::money::Cents;
//! let db = db::open_in_memory().unwrap();
//! setting::set(&db, key::PLANNING_TARGET, Cents::from_dollars(10_000)).unwrap();
//! let target: Option<Cents> = setting::get(&db, key::PLANNING_TARGET).unwrap();
//! assert_eq!(target, Some(Cents::from_dollars(10_000)));
//! ```
//!
//! so a percentage cannot be stored under a key that holds an amount:
//!
//! ```compile_fail
//! use mistermanager::db::{self, setting::{self, key}};
//! use mistermanager::rate::Percent;
//! let db = db::open_in_memory().unwrap();
//! setting::set(&db, key::PLANNING_TARGET, Percent(35)).unwrap();
//! ```

use super::date;
use super::{AccountId, Db, GoalId};
use crate::money::Cents;
use crate::rate::{BasisPoints, Percent};
use anyhow::{Context, Result, anyhow};
use chrono::NaiveDate;
use rusqlite::{OptionalExtension, params};
use std::fmt;
use std::marker::PhantomData;

/// A setting key, and the type of the value stored under it.
pub struct Key<T> {
    name: &'static str,
    value: PhantomData<T>,
}

impl<T> Key<T> {
    pub const fn new(name: &'static str) -> Self {
        Key {
            name,
            value: PhantomData,
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }
}

// Derived `Copy`/`Clone`/`Debug` would each demand the same bound of `T`,
// which is wrong: a `Key` holds no `T`, only the promise of one.
impl<T> Copy for Key<T> {}

impl<T> Clone for Key<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> fmt::Debug for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({:?})", self.name)
    }
}

impl<T> fmt::Display for Key<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name)
    }
}

/// A type that can be stored in the `setting` table.
///
/// `decode` describes the shape it expected and the text it got; naming the
/// key is [`get`]'s job, since only the caller knows it.
pub trait Value: Sized {
    fn encode(&self) -> String;
    fn decode(raw: &str) -> Result<Self>;
}

/// Integer-backed values: everything that is an `i64` under a newtype.
macro_rules! integer_value {
    ($type:ty, $wrap:expr, $unwrap:expr) => {
        impl Value for $type {
            fn encode(&self) -> String {
                #[allow(clippy::redundant_closure_call)]
                $unwrap(self).to_string()
            }

            fn decode(raw: &str) -> Result<Self> {
                let n: i64 = raw
                    .parse()
                    .map_err(|_| anyhow!("is not an integer: {raw:?}"))?;
                #[allow(clippy::redundant_closure_call)]
                Ok($wrap(n))
            }
        }
    };
}

integer_value!(i64, |n| n, |v: &i64| *v);
integer_value!(Cents, Cents, |v: &Cents| v.0);
integer_value!(Percent, Percent, |v: &Percent| v.0);
integer_value!(BasisPoints, BasisPoints, |v: &BasisPoints| v.0);
integer_value!(GoalId, GoalId, |v: &GoalId| v.0);
integer_value!(AccountId, AccountId, |v: &AccountId| v.0);

impl Value for String {
    fn encode(&self) -> String {
        self.clone()
    }

    fn decode(raw: &str) -> Result<Self> {
        Ok(raw.to_string())
    }
}

impl Value for NaiveDate {
    fn encode(&self) -> String {
        date::iso(*self)
    }

    fn decode(raw: &str) -> Result<Self> {
        date::parse(raw, 0).map_err(|_| anyhow!("is not a YYYY-MM-DD date: {raw:?}"))
    }
}

pub fn get<T: Value>(db: &Db, key: Key<T>) -> Result<Option<T>> {
    match get_raw(db, key.name)? {
        None => Ok(None),
        Some(raw) => T::decode(&raw)
            .map(Some)
            .with_context(|| format!("setting {key}")),
    }
}

/// [`get`], falling back to `default` when the key is unset.
///
/// A malformed *stored* value is still an error -- only an absent one takes
/// the fallback.
pub fn get_or<T: Value>(db: &Db, key: Key<T>, default: T) -> Result<T> {
    Ok(get(db, key)?.unwrap_or(default))
}

pub fn set<T: Value>(db: &Db, key: Key<T>, value: T) -> Result<()> {
    set_raw(db, key.name, &value.encode())
}

/// Remove a key, so it reads as unset.
///
/// Deliberately not an error when the key was never there: this is how a
/// feature is switched *off*, and both callers clear a pair of keys of which
/// one is routinely already absent. An absent key is the "not configured"
/// state every reader already handles.
pub fn clear<T>(db: &Db, key: Key<T>) -> Result<()> {
    db.conn
        .execute("DELETE FROM setting WHERE key = ?1", params![key.name])?;
    Ok(())
}

fn get_raw(db: &Db, key: &str) -> Result<Option<String>> {
    let value = db
        .conn
        .query_row(
            "SELECT value FROM setting WHERE key = ?1",
            params![key],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    Ok(value)
}

fn set_raw(db: &Db, key: &str, value: &str) -> Result<()> {
    db.conn.execute(
        "INSERT INTO setting (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Every setting the app reads or writes.
///
/// The gates' goal-id keys are deliberately not here: they belong to
/// `gate::Gate`, which pairs each with the goal-name substring it is matched
/// by. See `src/gate.rs`. Likewise the Planning lines' destination keys
/// belong to `plan_line::Line`. See `src/plan_line.rs`.
pub mod key {
    use super::Key;
    use crate::money::Cents;
    use crate::rate::{BasisPoints, Percent};
    use chrono::NaiveDate;

    /// `Constants!E2`.
    pub const TAX_RATE: Key<BasisPoints> = Key::new("tax.rate_bp");
    /// `Constants!G2`. The days between two paydays are
    /// `calc::period_days` of it rather than a key of their own: the sheet
    /// carries both in `G2` and `H2`, and two cells for one cadence can
    /// disagree.
    pub const PAY_PERIODS_PER_YEAR: Key<i64> = Key::new("pay.periods_per_year");
    /// `Constants!J2`.
    pub const WORKBOOK_TODAY: Key<NaiveDate> = Key::new("dates.workbook_today");
    /// `Constants!K2`.
    pub const BIRTH_DATE: Key<NaiveDate> = Key::new("dates.birth_date");

    /// `Planning!D1`.
    pub const PLANNING_TARGET: Key<Cents> = Key::new("planning.target");
    /// `Planning!J11`.
    pub const PLANNING_BUFFER: Key<Cents> = Key::new("planning.buffer");
    /// `Planning!D3`, `Excess (Fixed)`.
    pub const PINNED_EXCESS: Key<Cents> = Key::new("plan.pinned_excess_cents");
    /// When [`PINNED_EXCESS`] was pinned. Absent for a pin that came from the
    /// import, which transcribes `Planning!D3` and has no date to go with it,
    /// so the drift line must render without one.
    pub const PINNED_AT: Key<NaiveDate> = Key::new("plan.pinned_at");
    /// `Planning!E19`.
    pub const BILL_PAYMENT_CAP: Key<Cents> = Key::new("planning.bill_payment_cap");
    /// `Planning!E20`.
    pub const MOM_AND_DAD_ANNUAL: Key<Cents> = Key::new("planning.mom_and_dad_annual");
    /// `Planning!E24`.
    pub const GOALS_FLOOR: Key<Cents> = Key::new("planning.goals_floor");

    /// `Planning!F19`.
    pub const BILL_PAYMENT_PCT: Key<Percent> = Key::new("planning.bill_payment_pct");
    /// `Planning!F25` — the Future Housing share of the remainder.
    ///
    /// The stored string still says `down_payment`. It is the key an existing
    /// database already holds this value under, and a renamed key reads as
    /// "not configured", which would silently substitute the default.
    pub const SPLIT_FUTURE_HOUSING_PCT: Key<Percent> = Key::new("planning.split_down_payment_pct");
    /// `Planning!F26`.
    pub const SPLIT_RETIREMENT_PCT: Key<Percent> = Key::new("planning.split_retirement_pct");
    /// `Planning!F27`.
    pub const SPLIT_INVESTMENT_PCT: Key<Percent> = Key::new("planning.split_investment_pct");

    /// How far ahead a recurring transaction generates, in months, capped per
    /// recurring transaction by `recurring_txn.horizon`. Three by default: a
    /// workbook *is* a calendar year, but the app spans years, and these
    /// recurring transactions are not stable a year out.
    pub const RECURRING_TXN_HORIZON_MONTHS: Key<i64> = Key::new("recurring_txn.horizon_months");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn round_trips_every_value_type() {
        let db = db::open_in_memory().unwrap();

        assert_eq!(get(&db, key::PLANNING_TARGET).unwrap(), None);

        set(&db, key::PLANNING_TARGET, Cents::from_dollars(10_000)).unwrap();
        assert_eq!(
            get(&db, key::PLANNING_TARGET).unwrap(),
            Some(Cents::from_dollars(10_000))
        );

        set(&db, key::PAY_PERIODS_PER_YEAR, 26).unwrap();
        assert_eq!(get(&db, key::PAY_PERIODS_PER_YEAR).unwrap(), Some(26));

        set(&db, key::SPLIT_RETIREMENT_PCT, Percent(15)).unwrap();
        assert_eq!(
            get(&db, key::SPLIT_RETIREMENT_PCT).unwrap(),
            Some(Percent(15))
        );

        set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        assert_eq!(get(&db, key::TAX_RATE).unwrap(), Some(BasisPoints(625)));

        let d = NaiveDate::from_ymd_opt(2026, 8, 27).unwrap();
        set(&db, key::PINNED_AT, d).unwrap();
        assert_eq!(get(&db, key::PINNED_AT).unwrap(), Some(d));

        let note: Key<String> = Key::new("planning.pinned_note");
        set(&db, note, "hello".to_string()).unwrap();
        assert_eq!(get(&db, note).unwrap().as_deref(), Some("hello"));

        let goal: Key<GoalId> = Key::new("some.goal_id");
        set(&db, goal, GoalId(7)).unwrap();
        assert_eq!(get(&db, goal).unwrap(), Some(GoalId(7)));

        let account: Key<AccountId> = Key::new("some.account_id");
        set(&db, account, AccountId(3)).unwrap();
        assert_eq!(get(&db, account).unwrap(), Some(AccountId(3)));
    }

    #[test]
    fn malformed_stored_values_are_rejected_and_name_their_key() {
        let db = db::open_in_memory().unwrap();

        set_raw(&db, key::PLANNING_TARGET.name(), "not-a-number").unwrap();
        let err = get(&db, key::PLANNING_TARGET).unwrap_err();
        assert!(err.to_string().contains("planning.target"), "{err}");

        set_raw(&db, key::PAY_PERIODS_PER_YEAR.name(), "twenty-six").unwrap();
        assert!(get(&db, key::PAY_PERIODS_PER_YEAR).is_err());

        // US-style date, not the YYYY-MM-DD everything else stores.
        set_raw(&db, key::PINNED_AT.name(), "08/27/2026").unwrap();
        assert!(get(&db, key::PINNED_AT).is_err());
    }

    /// The fallback is for an *absent* key. A stored value that cannot be
    /// decoded must still be an error, or a corrupt setting would quietly
    /// become the default.
    #[test]
    fn get_or_takes_the_default_only_when_the_key_is_unset() {
        let db = db::open_in_memory().unwrap();
        assert_eq!(
            get_or(&db, key::PLANNING_TARGET, Cents::from_dollars(10_000)).unwrap(),
            Cents::from_dollars(10_000)
        );

        set(&db, key::PLANNING_TARGET, Cents::from_dollars(9_000)).unwrap();
        assert_eq!(
            get_or(&db, key::PLANNING_TARGET, Cents::from_dollars(10_000)).unwrap(),
            Cents::from_dollars(9_000)
        );

        set_raw(&db, key::PLANNING_TARGET.name(), "rubbish").unwrap();
        assert!(get_or(&db, key::PLANNING_TARGET, Cents::from_dollars(10_000)).is_err());
    }

    #[test]
    fn set_overwrites() {
        let db = db::open_in_memory().unwrap();
        let k: Key<i64> = Key::new("k");
        set(&db, k, 1).unwrap();
        set(&db, k, 2).unwrap();
        assert_eq!(get(&db, k).unwrap(), Some(2));
    }

    /// Unpinning has to remove the key, not write a sentinel: `compute` reads
    /// `Option<Cents>` and a zero pin is a legitimate value meaning "no excess
    /// this period".
    #[test]
    fn clearing_a_key_makes_it_read_as_unset() {
        let db = db::open_in_memory().unwrap();
        set(&db, key::PINNED_EXCESS, Cents::from_dollars(17_500)).unwrap();
        assert!(get(&db, key::PINNED_EXCESS).unwrap().is_some());

        clear(&db, key::PINNED_EXCESS).unwrap();

        assert_eq!(get(&db, key::PINNED_EXCESS).unwrap(), None);
    }

    /// Unpinning a plan that was never pinned is a no-op, not a failure --
    /// `p` is a toggle and the two keys are cleared together, so one of them
    /// is routinely already absent.
    #[test]
    fn clearing_a_key_that_was_never_set_is_not_an_error() {
        let db = db::open_in_memory().unwrap();
        clear(&db, key::PINNED_AT).unwrap();
        assert_eq!(get(&db, key::PINNED_AT).unwrap(), None);
    }

    #[test]
    fn clearing_one_key_leaves_its_neighbours_alone() {
        let db = db::open_in_memory().unwrap();
        let d = NaiveDate::from_ymd_opt(2026, 8, 14).unwrap();
        set(&db, key::PINNED_EXCESS, Cents::from_dollars(17_500)).unwrap();
        set(&db, key::PINNED_AT, d).unwrap();

        clear(&db, key::PINNED_EXCESS).unwrap();

        assert_eq!(get(&db, key::PINNED_AT).unwrap(), Some(d));
    }

    /// Every key is written once, here, so a duplicate would mean two
    /// settings silently overwriting each other.
    #[test]
    fn no_two_keys_share_a_name() {
        let mut names = vec![
            key::TAX_RATE.name(),
            key::PAY_PERIODS_PER_YEAR.name(),
            key::WORKBOOK_TODAY.name(),
            key::BIRTH_DATE.name(),
            key::PLANNING_TARGET.name(),
            key::PLANNING_BUFFER.name(),
            key::PINNED_EXCESS.name(),
            key::PINNED_AT.name(),
            key::BILL_PAYMENT_CAP.name(),
            key::MOM_AND_DAD_ANNUAL.name(),
            key::GOALS_FLOOR.name(),
            key::BILL_PAYMENT_PCT.name(),
            key::SPLIT_FUTURE_HOUSING_PCT.name(),
            key::SPLIT_RETIREMENT_PCT.name(),
            key::SPLIT_INVESTMENT_PCT.name(),
            key::RECURRING_TXN_HORIZON_MONTHS.name(),
        ];
        names.extend(crate::gate::Gate::ALL.iter().map(|g| g.key().name()));
        names.extend(
            crate::savings_block::Block::ALL
                .iter()
                .map(|b| b.key().name()),
        );
        names.extend(
            crate::default_source::Source::ALL
                .iter()
                .map(|s| s.key().name()),
        );
        // Only the keys `Line` owns: the two gate-backed lines borrow the
        // gate's key, and counting it twice would report a clash with itself.
        names.extend(
            crate::plan_line::Line::ALL
                .iter()
                .filter_map(|l| match l.destination() {
                    crate::plan_line::Destination::Goal(key) => l.owned_goal().map(|_| key.name()),
                    crate::plan_line::Destination::Account(key) => Some(key.name()),
                    crate::plan_line::Destination::Spread => None,
                }),
        );
        let before = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), before, "two settings share a key");
    }

    #[test]
    fn schema_is_applied_at_the_current_version() {
        // The real idempotency test -- that applying the schema twice does
        // not reapply it -- lives in `db::mod::tests`.
        let db = db::open_in_memory().unwrap();
        let version: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, db::SCHEMA_VERSION);
    }
}
