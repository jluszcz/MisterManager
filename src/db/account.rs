use super::{AccountId, Db};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::types::{FromSql, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::{OptionalExtension, Result as SqlResult, Row, ToSql, params};
use std::str::FromStr;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Kind {
    Cash,
    Credit,
}

impl Kind {
    /// Every kind, in the order the Accounts screen's `a` selector cycles
    /// them. Beside the enum rather than on the screen, for
    /// [`InterestPolicy::ALL`]'s reason: a screen offering a subset would
    /// leave a variant unreachable with nothing to say so.
    pub const ALL: [Kind; 2] = [Kind::Cash, Kind::Credit];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Cash => "cash",
            Kind::Credit => "credit",
        }
    }

    /// What the Overview labels this kind's total row, and what the Accounts
    /// screen's `Kind` column and its `a` selector call it.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Cash => "Cash",
            Kind::Credit => "Credit",
        }
    }
}

impl FromStr for Kind {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "cash" => Ok(Kind::Cash),
            "credit" => Ok(Kind::Credit),
            other => bail!("unknown account kind {other:?}"),
        }
    }
}

/// Which subtotal band an account sits in on the Overview.
///
/// A group subdivides exactly one [`Kind`]: cash splits into `Checking` and
/// `Savings`, and credit does not split, so `Credit` is the whole kind. The
/// variants are exactly the schema's `CHECK (grp IN (...))` list -- keep the
/// two in step, or an update that type-checks will fail against the
/// constraint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Group {
    Checking,
    Savings,
    Credit,
}

impl Group {
    pub fn as_str(self) -> &'static str {
        match self {
            Group::Checking => "checking",
            Group::Savings => "savings",
            Group::Credit => "credit",
        }
    }

    /// What the Overview labels this band's subtotal row.
    pub fn label(self) -> &'static str {
        match self {
            Group::Checking => "Checking",
            Group::Savings => "Savings",
            Group::Credit => "Credit",
        }
    }

    /// The kind this group subdivides. Every group belongs to exactly one,
    /// which is what lets `set_group` refuse a value disagreeing with the
    /// account's own `kind`.
    pub fn kind(self) -> Kind {
        match self {
            Group::Checking | Group::Savings => Kind::Cash,
            Group::Credit => Kind::Credit,
        }
    }

    /// The bands a kind offers, in Overview order.
    ///
    /// The inverse of [`Group::kind`], and what the Accounts screen's band
    /// selector cycles: offering only these is what stops the owner picking
    /// a band `set_group` would then refuse. The two must stay in step, and
    /// `every_band_belongs_to_the_kind_that_offers_it` is what says so.
    pub fn bands(kind: Kind) -> &'static [Group] {
        match kind {
            Kind::Cash => &[Group::Checking, Group::Savings],
            Kind::Credit => &[Group::Credit],
        }
    }
}

impl FromStr for Group {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "checking" => Ok(Group::Checking),
            "savings" => Ok(Group::Savings),
            "credit" => Ok(Group::Credit),
            other => bail!("unknown account group {other:?}"),
        }
    }
}

/// The band a freshly imported account starts in, and the only placement
/// rule there is: the workbook carries codes and nothing else, so where an
/// account sits on the Overview is the owner's to say on the Accounts
/// screen. Cash defaults to `Savings` rather than `Checking` because a
/// container added to the workbook is far likelier to be another savings pot
/// than a second current account.
fn default_group(kind: Kind) -> Group {
    match kind {
        Kind::Cash => Group::Savings,
        Kind::Credit => Group::Credit,
    }
}

/// Which prefill the allocation worksheet opens an interest posting with.
///
/// The variants are exactly the schema's `CHECK (interest_policy IN (...))`
/// list: keep the two in step, or an update that type-checks will fail
/// against the constraint.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum InterestPolicy {
    /// Split across the container's `interest_eligible` goals, by balance.
    ProRata,
    /// Reuse the container's previous interest posting, rescaled.
    Manual,
}

impl InterestPolicy {
    /// Every policy, in the order the Accounts screen's selector cycles them.
    ///
    /// Beside the enum rather than on the screen, for `fund::Target::KINDS`'s
    /// reason: a screen offering a subset would leave a variant unreachable
    /// with nothing to say so.
    pub const ALL: [InterestPolicy; 2] = [InterestPolicy::ProRata, InterestPolicy::Manual];

    pub fn as_str(self) -> &'static str {
        match self {
            InterestPolicy::ProRata => "pro_rata",
            InterestPolicy::Manual => "manual",
        }
    }

    /// What the Accounts screen calls this policy — what it *does*, not the
    /// string it is stored as.
    pub fn label(self) -> &'static str {
        match self {
            InterestPolicy::ProRata => "by balance",
            InterestPolicy::Manual => "like last time",
        }
    }
}

impl FromStr for InterestPolicy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "pro_rata" => Ok(InterestPolicy::ProRata),
            "manual" => Ok(InterestPolicy::Manual),
            other => bail!("unknown interest policy {other:?}"),
        }
    }
}

/// The color an account's name draws in, on every screen that names it.
///
/// The owner's, like the name beside it: the workbook carries a code and
/// nothing else, so nothing here is written by an import past the row's
/// first insert. Stored as `TEXT` and constrained by the schema's
/// `CHECK (color IN (...))`, the same construction as [`Kind`], [`Group`]
/// and [`InterestPolicy`] -- an index into a palette would leave the
/// database holding a number whose meaning lived in one array in `src/tui/`,
/// and reordering that array would silently repaint every account.
///
/// The names are what the Accounts screen's selector shows. What each one
/// actually looks like is `tui::style`'s to say, because
/// `ratatui::style::Color` is named there and nowhere else.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AccountColor {
    Blue,
    Copper,
    Violet,
    Teal,
    Rose,
    Olive,
    Indigo,
    Tan,
}

impl AccountColor {
    /// Every color, in the order the Accounts screen's selector cycles them.
    ///
    /// Beside the enum rather than on the screen, for [`InterestPolicy::ALL`]'s
    /// reason: a screen offering a subset would leave a variant unreachable
    /// with nothing to say so.
    pub const ALL: [AccountColor; 8] = [
        AccountColor::Blue,
        AccountColor::Copper,
        AccountColor::Violet,
        AccountColor::Teal,
        AccountColor::Rose,
        AccountColor::Olive,
        AccountColor::Indigo,
        AccountColor::Tan,
    ];

    /// The color an account nobody has picked one for is drawn in.
    ///
    /// Keyed on the id and not on the account's position in whatever list a
    /// screen is holding: `Ledger` is handed a kind-filtered list, so "the
    /// third account here" is a different account on Cash than it is on
    /// Credit, and the colors would disagree between two screens showing the
    /// same ledger.
    ///
    /// Here rather than in `tui::style` because it is a fact about *this*
    /// enum -- which variant an id lands on -- and nothing about what a
    /// variant looks like. The Accounts screen reads it too, to open its
    /// selector on the color an unset account is already being drawn in.
    ///
    /// `rem_euclid` rather than `%` so a negative id -- which `account.id`
    /// being a rowid rules out, but the type does not -- wraps into the list
    /// instead of panicking on an index out of range. Ids run 1..n after an
    /// import, so accounts take distinct colors until there are more than
    /// [`AccountColor::ALL`] holds.
    pub fn derived(id: AccountId) -> AccountColor {
        AccountColor::ALL[id.0.rem_euclid(AccountColor::ALL.len() as i64) as usize]
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AccountColor::Blue => "blue",
            AccountColor::Copper => "copper",
            AccountColor::Violet => "violet",
            AccountColor::Teal => "teal",
            AccountColor::Rose => "rose",
            AccountColor::Olive => "olive",
            AccountColor::Indigo => "indigo",
            AccountColor::Tan => "tan",
        }
    }

    /// What the Accounts screen calls this color. Capitalized because it is
    /// a name on a form rather than the string it is stored as.
    pub fn label(self) -> &'static str {
        match self {
            AccountColor::Blue => "Blue",
            AccountColor::Copper => "Copper",
            AccountColor::Violet => "Violet",
            AccountColor::Teal => "Teal",
            AccountColor::Rose => "Rose",
            AccountColor::Olive => "Olive",
            AccountColor::Indigo => "Indigo",
            AccountColor::Tan => "Tan",
        }
    }
}

impl FromStr for AccountColor {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        AccountColor::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .with_context(|| format!("unknown account color {s:?}"))
    }
}

/// The two pieces of text an account is named by, each its own type.
///
/// Neither implements `Display`, and that absence is the whole point. Every
/// account that reaches a screen with no color on it gets there through a
/// `format!` -- a `String` cannot carry a tint, so an account that becomes
/// one has lost its color before any render function can see it. With no
/// `Display`, that does not compile, and the only route from an account to a
/// glyph is `tui::Account`, which colors what it draws.
///
/// `as_str` is the deliberate escape, for the handful of uses that are not
/// displays of an account: a description prefill, a search filter folding
/// case, a form seeding its editable field. It is named plainly rather than
/// hidden, so reaching for it is visible in a diff.
///
/// `PartialEq<&str>` so an assertion reads as it always did, and
/// `ToSql`/`FromSql` so `from_row` and every query are untouched.
macro_rules! account_text {
    ($name:ident, $field:ident, $what:literal) => {
        #[doc = concat!("An account's ", $what, ".")]
        ///
        /// Text the database stores, and deliberately **not** something
        /// `format!` will take:
        ///
        /// ```compile_fail
        /// use mistermanager::db::{self, account::Kind};
        /// let db = db::open_in_memory().unwrap();
        /// let id = db::account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        /// let account = db::account::get(&db, id).unwrap();
        #[doc = concat!("println!(\"{}\", account.", stringify!($field), ");")]
        /// ```
        #[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(String);

        impl $name {
            /// The text, with no color on it.
            ///
            /// Not for drawing an account -- `tui::Account` draws one, and it
            /// colors what it draws. This is for the uses that are not
            /// displays at all.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> $name {
                $name(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> $name {
                $name(s)
            }
        }

        impl PartialEq<&str> for $name {
            fn eq(&self, other: &&str) -> bool {
                self.0 == *other
            }
        }

        impl ToSql for $name {
            fn to_sql(&self) -> SqlResult<ToSqlOutput<'_>> {
                self.0.to_sql()
            }
        }

        impl FromSql for $name {
            fn column_result(value: ValueRef<'_>) -> FromSqlResult<$name> {
                String::column_result(value).map($name)
            }
        }
    };
}

account_text!(
    AccountCode,
    code,
    "short code, as the workbook's `Constants` sheet carries it"
);
account_text!(
    AccountName,
    name,
    "name, which is the owner's rather than the workbook's"
);

#[derive(Clone, Debug)]
pub struct Account {
    pub id: AccountId,
    pub code: AccountCode,
    pub name: AccountName,
    pub kind: Kind,
    pub sort: i64,
    pub group: Group,
    /// The color the owner picked, or `None` for an account nobody has
    /// picked one for -- which `tui::style::account_color` draws in the
    /// shade the id derives, so accounts are distinguishable the moment
    /// they are imported rather than only once they are configured.
    pub color: Option<AccountColor>,
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<Account> {
    let kind: String = row.get(3)?;
    let group: String = row.get(5)?;
    let color: Option<String> = row.get(6)?;
    Ok(Account {
        id: row.get(0)?,
        code: row.get(1)?,
        name: row.get(2)?,
        kind: kind.parse().expect("schema CHECK guarantees a valid kind"),
        sort: row.get(4)?,
        group: group
            .parse()
            .expect("schema CHECK guarantees a valid group"),
        color: color.map(|c| {
            c.parse()
                .expect("schema CHECK guarantees a valid account color")
        }),
    })
}

/// A `SELECT` of the columns [`from_row`] reads, in the order it reads them,
/// with `$tail` appended. One list per table -- see [`crate::db`] for the
/// idiom.
macro_rules! select_account {
    ($tail:literal) => {
        concat!(
            "SELECT id, code, name, kind, sort, grp, color FROM account ",
            $tail
        )
    };
}

/// Inserts an account in its kind's default group. Placing it in the other
/// cash band is `set_group`'s job, so that the one guarded write is the only
/// way `grp` and `kind` can ever come to disagree -- and it refuses.
///
/// A code the kind already holds is refused here rather than left to the
/// schema's `account_code_kind`, because the code is typed by hand on the
/// Accounts screen: a constraint failure names the index, where the owner
/// needs to be told which code they just retyped. Per kind, not per code --
/// one code naming both a cash account and the card drawn on it is what the
/// index is keyed that way to allow. `import::constants` skips a code the
/// kind already holds before it gets here, so an import never meets this.
///
/// The clash is reported as the code the database holds rather than the one
/// that was typed, because [`by_code`] folds case: an owner told `"chk"`
/// already exists, having typed exactly that, has been told nothing.
pub fn insert(db: &Db, code: &str, name: &str, kind: Kind, sort: i64) -> Result<AccountId> {
    if let Some(existing) = by_code(db, code, kind)? {
        bail!(
            "a {} account with code {:?} already exists",
            kind.as_str(),
            existing.code
        );
    }
    db.conn.execute(
        "INSERT INTO account (code, name, kind, sort, grp) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            code,
            name,
            kind.as_str(),
            sort,
            default_group(kind).as_str()
        ],
    )?;
    Ok(AccountId(db.conn.last_insert_rowid()))
}

/// The account a code names, if the kind holds one.
///
/// Matched case-insensitively, and that is what makes adoption work rather
/// than a convenience: the code is typed by hand on the Accounts screen and
/// read off the sheet by the import, and the two have to meet. An account
/// entered as `chk` against a sheet that later grows `CHK` would otherwise
/// pass `insert`'s guard, miss `import::constants`' skip, and end up as a
/// second row for one real account with the balance split between them and no
/// way to merge the halves. Folding the case is what `account_code_kind`
/// exists to keep the database in step with.
pub fn by_code(db: &Db, code: &str, kind: Kind) -> Result<Option<Account>> {
    let found = db
        .conn
        .query_row(
            select_account!("WHERE code = ?1 COLLATE NOCASE AND kind = ?2"),
            params![code, kind.as_str()],
            from_row,
        )
        .optional()?;
    Ok(found)
}

pub fn list(db: &Db) -> Result<Vec<Account>> {
    let mut stmt = db
        .conn
        .prepare(select_account!("ORDER BY kind, sort, code"))?;
    let rows = stmt.query_map([], from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn list_by_kind(db: &Db, kind: Kind) -> Result<Vec<Account>> {
    let mut stmt = db
        .conn
        .prepare(select_account!("WHERE kind = ?1 ORDER BY sort, code"))?;
    let rows = stmt.query_map(params![kind.as_str()], from_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// One account by id. A missing account is an error, not `None`: an id read
/// off another row is a foreign key, and a dangling one is a corrupt
/// database.
pub fn get(db: &Db, id: AccountId) -> Result<Account> {
    db.conn
        .query_row(select_account!("WHERE id = ?1"), params![id], from_row)
        .optional()?
        .with_context(|| format!("no account with id {id}"))
}

/// Moves an account into `group`.
///
/// `grp` and `kind` say overlapping things and the schema constrains them
/// separately, so this is where they are held together: a group belonging to
/// the other kind is refused rather than written. Without the guard a cash
/// account could claim the credit band and be subtotalled into the wrong
/// half of Net.
pub fn set_group(db: &Db, id: AccountId, group: Group) -> Result<()> {
    let account = get(db, id)?;
    ensure!(
        group.kind() == account.kind,
        "account {id} is {}, so it cannot join the {} group",
        account.kind.as_str(),
        group.as_str()
    );
    db.conn.execute(
        "UPDATE account SET grp = ?2 WHERE id = ?1",
        params![id, group.as_str()],
    )?;
    Ok(())
}

/// The current account: the one cash account in the `Checking` band.
///
/// Which account that is cannot come from the workbook -- the sheet carries
/// codes and no band -- so the Accounts screen's `Band` field is what says
/// so, and this is the one place that reading happens. Two things depend on
/// it and must not disagree: the Planning waterfall's `Excess (Actual)` is
/// this account's balance, and `transfer::source` is where every payday
/// transfer leaves from. A second derivation is how those two would come to
/// mean different accounts.
///
/// Exactly one, because a transfer leaves from one account. **None** is a
/// database nobody has finished configuring -- a fresh import puts every cash
/// account in its kind's default band, which is `Savings` -- and **more than
/// one** is an ambiguity only the owner can settle. Both are errors saying
/// what to do rather than a pick made on the caller's behalf: the Planning
/// screen renders the message in place of the plan, exactly as it does for
/// every other reason a plan will not resolve.
pub fn checking(db: &Db) -> Result<Account> {
    let mut found: Vec<Account> = list_by_kind(db, Kind::Cash)?
        .into_iter()
        .filter(|a| a.group == Group::Checking)
        .collect();
    ensure!(
        !found.is_empty(),
        "no account is in the Checking band -- press 9 and put the current account there"
    );
    ensure!(
        found.len() == 1,
        "{} accounts are in the Checking band ({}), and a transfer leaves from one -- \
         press 9 and move all but one to Savings",
        found.len(),
        found
            .iter()
            // names the offenders in the ambiguity error, not a display of an account
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(found.remove(0))
}

/// Renames an account.
///
/// The workbook carries a short code and nothing else, so the name every
/// screen shows is the owner's, typed on the Accounts screen and surviving a
/// `--replace` -- `account` is not an imported table.
pub fn set_name(db: &Db, id: AccountId, name: &str) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE account SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    ensure!(changed == 1, "no account with id {id}");
    Ok(())
}

/// Moves an account to `position` among the accounts of its kind, and
/// renumbers `sort` over all of them so the column stays `0..n-1`.
///
/// A position rather than a raw `sort`, because `sort` is only ever read
/// through an `ORDER BY` that breaks ties by code: "set sort to 2" is an
/// instruction whose result depends on rows the caller never saw, where
/// "put it third" is not. Renumbering the whole kind is what makes the order
/// the screen shows the order that is stored.
///
/// A position past the end lands last rather than erroring: the screen's
/// selector cannot produce one, and clamping is the same answer a drag past
/// the bottom of a list gives.
pub fn reorder(db: &Db, id: AccountId, position: usize) -> Result<()> {
    db.transaction(|db| {
        let account = get(db, id)?;
        let mut ordered = list_by_kind(db, account.kind)?;
        let from = ordered
            .iter()
            .position(|a| a.id == id)
            .expect("the account was just read by id, so its kind lists it");
        let moved = ordered.remove(from);
        ordered.insert(position.min(ordered.len()), moved);
        for (sort, account) in ordered.iter().enumerate() {
            db.conn.execute(
                "UPDATE account SET sort = ?2 WHERE id = ?1",
                params![account.id, sort as i64],
            )?;
        }
        Ok(())
    })
}

/// The container's interest policy. `NULL` reads as `Manual`: an unconfigured
/// container is one whose split is a judgment call, which is the safe default
/// -- computing one the owner never chose is the failure worth avoiding. It
/// is set on the Accounts screen, and survives a `--replace` with the rest of
/// the row.
///
/// A missing account is an error, not a default: an id read off a goal that
/// names no account is a corrupt database.
pub fn interest_policy(db: &Db, id: AccountId) -> Result<InterestPolicy> {
    let raw: Option<String> = db
        .conn
        .query_row(
            "SELECT interest_policy FROM account WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )
        .optional()?
        .with_context(|| format!("no account with id {id}"))?;
    match raw {
        None => Ok(InterestPolicy::Manual),
        Some(text) => text.parse(),
    }
}

pub fn set_interest_policy(db: &Db, id: AccountId, policy: InterestPolicy) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE account SET interest_policy = ?2 WHERE id = ?1",
        params![id, policy.as_str()],
    )?;
    ensure!(changed == 1, "no account with id {id}");
    Ok(())
}

/// Sets the color an account's name draws in, or clears it back to the shade
/// its id derives.
///
/// `None` is a real choice rather than a missing one: it is what every
/// account starts at, and what the Accounts screen's selector calls `—`.
/// Clearing it therefore restores exactly what an unconfigured database
/// shows, which is what makes the whole field an override rather than a
/// step the owner has to complete before the screens read properly.
pub fn set_color(db: &Db, id: AccountId, color: Option<AccountColor>) -> Result<()> {
    let changed = db.conn.execute(
        "UPDATE account SET color = ?2 WHERE id = ?1",
        params![id, color.map(AccountColor::as_str)],
    )?;
    ensure!(changed == 1, "no account with id {id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[test]
    fn group_as_str_and_from_str_round_trip() {
        for group in [Group::Checking, Group::Savings, Group::Credit] {
            assert_eq!(group.as_str().parse::<Group>().unwrap(), group);
        }
        assert!("chequing".parse::<Group>().is_err());
    }

    /// The enum and the schema's `CHECK (grp IN (...))` are two independent
    /// lists of the same three strings. A variant missing from the constraint
    /// type-checks and then fails at runtime.
    #[test]
    fn every_group_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        for group in [Group::Checking, Group::Savings, Group::Credit] {
            let id = insert(&db, "X", "X", group.kind(), 0).unwrap();
            set_group(&db, id, group)
                .unwrap_or_else(|e| panic!("{group:?} is not in the schema's CHECK list: {e}"));
            assert_eq!(get(&db, id).unwrap().group, group);
            db.conn
                .execute("DELETE FROM account WHERE id = ?1", params![id])
                .unwrap();
        }
    }

    /// A group subdivides exactly one kind. Cash splits in two; credit does
    /// not split, so `Group::Credit` is the whole kind.
    #[test]
    fn a_group_belongs_to_the_kind_it_subdivides() {
        assert_eq!(Group::Checking.kind(), Kind::Cash);
        assert_eq!(Group::Savings.kind(), Kind::Cash);
        assert_eq!(Group::Credit.kind(), Kind::Credit);
    }

    /// Nothing in the schema stops a cash row claiming `credit`, so the
    /// writer is the guard: the two columns cannot drift because the only
    /// way to set one rejects a value disagreeing with the other.
    #[test]
    fn set_group_rejects_a_group_from_the_other_kind() {
        let db = db::open_in_memory().unwrap();
        let checking = insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let err = set_group(&db, checking, Group::Credit).unwrap_err();
        assert!(err.to_string().contains("credit"), "{err}");
        assert_eq!(get(&db, checking).unwrap().group, Group::Savings);
    }

    /// The band selector offers exactly the bands `set_group` will accept,
    /// so a pick on the Accounts screen cannot be refused by the write it
    /// leads to. The two lists are independent matches over the same fact.
    #[test]
    fn every_band_belongs_to_the_kind_that_offers_it() {
        for kind in [Kind::Cash, Kind::Credit] {
            let bands = Group::bands(kind);
            assert!(!bands.is_empty(), "{kind:?} offers no band");
            for band in bands {
                assert_eq!(band.kind(), kind, "{band:?} is not a {kind:?} band");
            }
        }
        // And every band is offered by the kind it subdivides -- a variant
        // missing from `bands` would be unreachable from the screen.
        for band in [Group::Checking, Group::Savings, Group::Credit] {
            assert!(
                Group::bands(band.kind()).contains(&band),
                "{band:?} is offered by no kind"
            );
        }
    }

    /// The workbook carries a code and nothing else, so the name is the
    /// owner's -- and `--replace` does not touch `account`, so it stays.
    #[test]
    fn an_account_can_be_renamed() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "SAV", "SAV", Kind::Cash, 0).unwrap();
        set_name(&db, id, "Rainy Day").unwrap();
        assert_eq!(get(&db, id).unwrap().name, "Rainy Day");
    }

    #[test]
    fn renaming_a_missing_account_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let err = set_name(&db, AccountId(999), "Rainy Day").unwrap_err();
        assert!(err.to_string().contains("999"), "{err}");
    }

    /// `reorder` takes a position and renumbers the whole kind, so what the
    /// screen shows is what is stored -- no ties for the code to break.
    #[test]
    fn reorder_moves_an_account_and_renumbers_its_kind() {
        let db = db::open_in_memory().unwrap();
        let chk = insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let sav = insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let bkr = insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();

        reorder(&db, bkr, 0).unwrap();

        let ids: Vec<AccountId> = list_by_kind(&db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, vec![bkr, chk, sav]);
        let sorts: Vec<i64> = list_by_kind(&db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.sort)
            .collect();
        assert_eq!(sorts, vec![0, 1, 2]);
    }

    /// One kind's order is not the other's: renumbering cash must leave the
    /// cards exactly where they were.
    #[test]
    fn reorder_leaves_the_other_kind_alone() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let sav = insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let one = insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
        let two = insert(&db, "CC2", "Card Two", Kind::Credit, 1).unwrap();

        reorder(&db, sav, 0).unwrap();

        let cards: Vec<AccountId> = list_by_kind(&db, Kind::Credit)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(cards, vec![one, two]);
    }

    /// A position past the end lands last. The selector cannot produce one,
    /// but clamping is the answer a drag past the bottom of a list gives.
    #[test]
    fn reorder_past_the_end_lands_last() {
        let db = db::open_in_memory().unwrap();
        let chk = insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let sav = insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();

        reorder(&db, chk, 99).unwrap();

        let ids: Vec<AccountId> = list_by_kind(&db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect();
        assert_eq!(ids, vec![sav, chk]);
    }

    /// An account nobody has placed sits in its kind's default band, so a
    /// code the workbook grows before the owner names it still lands
    /// somewhere sensible rather than nowhere.
    #[test]
    fn an_inserted_account_takes_its_kinds_default_group() {
        let db = db::open_in_memory().unwrap();
        let cash = insert(&db, "NEW", "NEW", Kind::Cash, 0).unwrap();
        let card = insert(&db, "NEW", "NEW", Kind::Credit, 0).unwrap();
        assert_eq!(get(&db, cash).unwrap().group, Group::Savings);
        assert_eq!(get(&db, card).unwrap().group, Group::Credit);
    }

    #[test]
    fn kind_as_str_and_from_str_round_trip() {
        assert_eq!(Kind::Cash.as_str(), "cash");
        assert_eq!(Kind::Credit.as_str(), "credit");
        assert_eq!("cash".parse::<Kind>().unwrap(), Kind::Cash);
        assert_eq!("credit".parse::<Kind>().unwrap(), Kind::Credit);
    }

    #[test]
    fn from_str_rejects_an_unknown_kind() {
        assert!("bogus".parse::<Kind>().is_err());
    }

    #[test]
    fn by_code_discriminates_by_kind() {
        let db = db::open_in_memory().unwrap();
        let cash_id = insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let credit_id = insert(&db, "CHK", "Everyday Card", Kind::Credit, 0).unwrap();
        // The same code exists for both kinds -- the one-code-two-accounts
        // design `UNIQUE (code, kind)` rests on.
        assert_ne!(cash_id, credit_id);

        let cash = by_code(&db, "CHK", Kind::Cash).unwrap().unwrap();
        assert_eq!(cash.id, cash_id);
        assert_eq!(cash.kind, Kind::Cash);

        let credit = by_code(&db, "CHK", Kind::Credit).unwrap().unwrap();
        assert_eq!(credit.id, credit_id);
        assert_eq!(credit.kind, Kind::Credit);

        assert!(by_code(&db, "Nope", Kind::Cash).unwrap().is_none());
    }

    #[test]
    fn list_orders_by_kind_then_sort_then_code() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "ZZZ", "Z Credit", Kind::Credit, 5).unwrap();
        insert(&db, "AAA", "A Cash", Kind::Cash, 1).unwrap();
        insert(&db, "BBB", "B Cash", Kind::Cash, 0).unwrap();
        insert(&db, "CCC", "C Credit", Kind::Credit, 0).unwrap();

        let codes: Vec<String> = list(&db)
            .unwrap()
            .into_iter()
            .map(|a| a.code.as_str().to_string())
            .collect();
        // "cash" < "credit" alphabetically, so all cash accounts sort first;
        // within a kind, sort then code.
        assert_eq!(codes, vec!["BBB", "AAA", "CCC", "ZZZ"]);
    }

    #[test]
    fn list_by_kind_filters_and_orders_by_sort_then_code() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "ZZZ", "Z Credit", Kind::Credit, 5).unwrap();
        insert(&db, "AAA", "A Cash", Kind::Cash, 1).unwrap();
        insert(&db, "BBB", "B Cash", Kind::Cash, 0).unwrap();
        insert(&db, "CCC", "C Credit", Kind::Credit, 0).unwrap();

        let cash_codes: Vec<String> = list_by_kind(&db, Kind::Cash)
            .unwrap()
            .into_iter()
            .map(|a| a.code.as_str().to_string())
            .collect();
        assert_eq!(cash_codes, vec!["BBB", "AAA"]);

        let credit_codes: Vec<String> = list_by_kind(&db, Kind::Credit)
            .unwrap()
            .into_iter()
            .map(|a| a.code.as_str().to_string())
            .collect();
        assert_eq!(credit_codes, vec!["CCC", "ZZZ"]);
    }

    /// The enum and the schema's `CHECK (interest_policy IN (...))` are two
    /// independent lists of the same two strings. A variant missing from the
    /// constraint type-checks and then fails at runtime.
    #[test]
    fn every_interest_policy_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        let savings = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        for policy in [InterestPolicy::ProRata, InterestPolicy::Manual] {
            set_interest_policy(&db, savings, policy)
                .unwrap_or_else(|e| panic!("{policy:?} is not in the schema's CHECK list: {e}"));
            assert_eq!(interest_policy(&db, savings).unwrap(), policy);
        }
    }

    /// An account nobody has configured is `manual`: a container whose split
    /// is a judgment call would otherwise silently compute one and post an
    /// interest batch the owner never chose.
    #[test]
    fn an_unset_interest_policy_reads_as_manual() {
        let db = db::open_in_memory().unwrap();
        let savings = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        assert_eq!(
            interest_policy(&db, savings).unwrap(),
            InterestPolicy::Manual
        );
    }

    /// A dangling id is a corrupt database, not an unconfigured account.
    /// Reporting `manual` there would hide it behind a working default.
    #[test]
    fn the_interest_policy_of_a_missing_account_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let err = interest_policy(&db, AccountId(999)).unwrap_err();
        assert!(err.to_string().contains("999"), "{err}");
    }

    /// The enum and the schema's `CHECK (color IN (...))` are two independent
    /// lists of the same eight strings. A variant missing from the constraint
    /// type-checks and then fails at runtime.
    #[test]
    fn every_account_color_satisfies_the_schema_constraint() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        for color in AccountColor::ALL {
            set_color(&db, id, Some(color))
                .unwrap_or_else(|e| panic!("{color:?} is not in the schema's CHECK list: {e}"));
            assert_eq!(get(&db, id).unwrap().color, Some(color));
        }
    }

    /// Every existing row is in this state and stays in it until the owner
    /// says otherwise, so it has to be a state the screens can draw rather
    /// than a hole they have to work around.
    #[test]
    fn an_account_nobody_has_colored_holds_no_color() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        assert_eq!(get(&db, id).unwrap().color, None);
    }

    /// `—` on the selector is a choice, not the absence of one: picking it
    /// has to put an account back exactly where it started.
    #[test]
    fn a_color_can_be_cleared_back_to_the_derived_one() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        set_color(&db, id, Some(AccountColor::Teal)).unwrap();
        set_color(&db, id, None).unwrap();
        assert_eq!(get(&db, id).unwrap().color, None);
    }

    #[test]
    fn coloring_a_missing_account_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let err = set_color(&db, AccountId(999), Some(AccountColor::Blue)).unwrap_err();
        assert!(err.to_string().contains("999"), "{err}");
    }

    #[test]
    fn account_color_as_str_and_from_str_round_trip() {
        for color in AccountColor::ALL {
            assert_eq!(color.as_str().parse::<AccountColor>().unwrap(), color);
        }
        assert!("chartreuse".parse::<AccountColor>().is_err());
    }

    /// `ALL` is what the selector cycles, so a duplicate would offer one
    /// color twice and a short list would leave a variant unreachable.
    #[test]
    fn every_account_color_is_offered_exactly_once() {
        let mut seen: Vec<&str> = AccountColor::ALL.iter().map(|c| c.as_str()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), AccountColor::ALL.len());
    }

    #[test]
    fn interest_policy_as_str_and_from_str_round_trip() {
        for policy in [InterestPolicy::ProRata, InterestPolicy::Manual] {
            assert_eq!(policy.as_str().parse::<InterestPolicy>().unwrap(), policy);
        }
        assert!("prorata".parse::<InterestPolicy>().is_err());
    }

    /// The names go to SQLite as text and come back as the same text. A `ToSql`
    /// that stored the debug form would put `AccountName("Rainy Day")` in the
    /// column, and every `by_code` lookup and every screen would read it back.
    #[test]
    fn an_accounts_name_and_code_survive_a_round_trip_through_the_database() {
        let db = db::open_in_memory().unwrap();
        let id = insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let account = get(&db, id).unwrap();
        assert_eq!(account.code, "SAV");
        assert_eq!(account.name, "Rainy Day");
        assert_eq!(account.name.as_str(), "Rainy Day");
    }

    /// The schema's `account_code_kind` is the backstop; this is the
    /// sentence in front of it. A code is typed by hand on the Accounts
    /// screen now, so the collision is reachable from a modal -- and a raw
    /// constraint failure names the index rather than the code the owner
    /// just typed.
    #[test]
    fn insert_refuses_a_code_the_kind_already_holds() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let err = insert(&db, "SAV", "Nest Egg", Kind::Cash, 1).unwrap_err();
        assert!(err.to_string().contains("SAV"), "{err}");
        assert_eq!(list_by_kind(&db, Kind::Cash).unwrap().len(), 1);
    }

    /// Case is folded, because the code is typed by hand here and read off
    /// the sheet by the import, and adoption is those two meeting. `chk`
    /// entered against a sheet that later grows `CHK` would otherwise become
    /// a second row for one real account. The clash names the code on record
    /// rather than the one typed: told `"chk"` already exists, an owner who
    /// typed exactly that has been told nothing.
    #[test]
    fn insert_refuses_a_code_the_kind_holds_in_another_case() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        let err = insert(&db, "chk", "Everyday Again", Kind::Cash, 1).unwrap_err();
        assert!(err.to_string().contains("CHK"), "{err}");
        assert_eq!(list_by_kind(&db, Kind::Cash).unwrap().len(), 1);
        // And the import finds that row by the case the sheet spells it in,
        // which is what makes it an adoption rather than a duplicate.
        assert_eq!(
            by_code(&db, "chk", Kind::Cash).unwrap().unwrap().name,
            "Everyday"
        );
    }

    /// The refusal is per kind, because the constraint is: one code naming
    /// both a cash account and the card drawn on it is exactly what
    /// `UNIQUE (code, kind)` exists to allow.
    #[test]
    fn one_code_may_name_a_cash_account_and_a_card() {
        let db = db::open_in_memory().unwrap();
        insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        insert(&db, "CHK", "Everyday Card", Kind::Credit, 0).unwrap();
        assert_eq!(
            by_code(&db, "CHK", Kind::Cash).unwrap().unwrap().name,
            "Everyday"
        );
        assert_eq!(
            by_code(&db, "CHK", Kind::Credit).unwrap().unwrap().name,
            "Everyday Card"
        );
    }
}
