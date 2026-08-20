-- The frozen baseline: the schema as it stood at version 1.
--
-- **Do not edit this to describe a schema change.** A change is an arm in
-- `db::migration::MIGRATIONS`, which every database replays from here --
-- including a fresh one, which takes this file and then the whole chain.
-- Editing the baseline instead would give a new database a schema no existing
-- one can reach, which is the single failure the arrangement exists to rule
-- out.
--
-- It is rewritten only by a squash: with the owner's database at the head
-- version, the schema it actually has is dumped here as the new version 1 and
-- the chain is emptied. Between squashes this file describes version 1 and
-- the chain describes everything since.
--
-- Tables are declared in dependency order, so every `REFERENCES` names a
-- table that already exists.

CREATE TABLE account (
  id   INTEGER PRIMARY KEY,
  code TEXT    NOT NULL,
  name TEXT    NOT NULL,
  kind TEXT    NOT NULL CHECK (kind IN ('cash', 'credit')),
  -- Which subtotal band the Overview stacks this account in. A group
  -- subdivides exactly one kind, which `account::set_group` is what enforces:
  -- the two columns say overlapping things and the schema constrains them
  -- separately, so a cash row could otherwise claim the credit band and be
  -- subtotalled into the wrong half of Net. `grp`, not `group`: `GROUP` is a
  -- SQL keyword.
  grp  TEXT    NOT NULL CHECK (grp IN ('checking', 'savings', 'credit')),
  sort INTEGER NOT NULL DEFAULT 0,
  -- Which prefill the allocation worksheet opens an interest posting with.
  -- It lives on the account rather than in `setting` because every setting
  -- key is a `Key<T>` constant and a per-container key cannot be one. NULL
  -- reads as 'manual'. Set on the Accounts screen, like `name`, `grp` and
  -- `sort` beside it -- none of the four is written by the import past the
  -- row's first insert.
  interest_policy TEXT CHECK (interest_policy IN ('pro_rata', 'manual')),
  -- One bank can be both a checking account and a credit card, so code alone
  -- is not unique.
  UNIQUE (code, kind)
);

CREATE TABLE recurring_txn (
  id          INTEGER PRIMARY KEY,
  description TEXT    NOT NULL,
  cents       INTEGER NOT NULL,
  account_id  INTEGER NOT NULL REFERENCES account(id),
  cadence     TEXT    NOT NULL CHECK (cadence IN ('biweekly', 'monthly')),
  anchor_date TEXT    NOT NULL,
  -- A cap: the last date this one ever generates, or NULL for one that does
  -- not end.
  horizon     TEXT,
  -- A floor: how far past the rolling horizon the owner has asked this one to
  -- be written out. NULL until the first `x`. The two dates pull opposite
  -- ways, which is why they are two columns.
  generate_through TEXT,
  is_paycheck INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE txn (
  id          INTEGER PRIMARY KEY,
  date        TEXT    NOT NULL,
  cents       INTEGER NOT NULL,
  account_id  INTEGER NOT NULL REFERENCES account(id),
  description TEXT    NOT NULL,
  recurring_txn_id INTEGER REFERENCES recurring_txn(id),
  -- Set when a recurring transaction's row has been hand-edited, so
  -- regeneration leaves it alone.
  edited      INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX txn_account_date ON txn (account_id, date);
CREATE INDEX txn_description  ON txn (description);

CREATE TABLE recurring_goal (
  id         INTEGER PRIMARY KEY,
  name       TEXT    NOT NULL,
  month      INTEGER NOT NULL CHECK (month BETWEEN 1 AND 12),
  base_cents INTEGER NOT NULL,
  taxed      INTEGER NOT NULL DEFAULT 0,
  -- The workbook says "biannual" but means every two years.
  cadence    TEXT    NOT NULL CHECK (cadence IN ('annual', 'biennial'))
);

CREATE TABLE goal (
  id                   INTEGER PRIMARY KEY,
  name                 TEXT    NOT NULL,
  container_account_id INTEGER NOT NULL REFERENCES account(id),
  goal_cents           INTEGER NOT NULL,
  goal_date            TEXT,
  recurring_goal_id    INTEGER REFERENCES recurring_goal(id),
  interest_eligible    INTEGER NOT NULL DEFAULT 1,
  closed               INTEGER NOT NULL DEFAULT 0,
  sort                 INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX goal_container ON goal (container_account_id);

CREATE TABLE batch (
  id   INTEGER PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('paycheck', 'interest', 'adhoc', 'import')),
  date TEXT NOT NULL
);

CREATE TABLE allocation (
  id       INTEGER PRIMARY KEY,
  goal_id  INTEGER NOT NULL REFERENCES goal(id) ON DELETE CASCADE,
  date     TEXT    NOT NULL,
  cents    INTEGER NOT NULL,
  note     TEXT,
  batch_id INTEGER REFERENCES batch(id)
);

CREATE INDEX allocation_goal  ON allocation (goal_id);
CREATE INDEX allocation_batch ON allocation (batch_id);

-- The monthly bill block, `Planning!C6:E12`. Two categories rather than one
-- list with a flag, because `calc::planning` reports the two subtotals
-- separately and only the housing one reaches `lines.current_housing`.
CREATE TABLE bill (
  id       INTEGER PRIMARY KEY,
  label    TEXT    NOT NULL,
  cents    INTEGER NOT NULL,
  category TEXT    NOT NULL CHECK (category IN ('housing', 'other')),
  sort     INTEGER NOT NULL DEFAULT 0
);

-- The asset-allocation block, `Planning!I1:M5`.
--
-- `share_bp` is NULL for the row whose target tracks age and basis points for
-- a row taking a share of what age leaves, which is what `fund::Target` says
-- in Rust. The second CHECK pairs the two columns so the schema says it too:
-- without it a `remainder_share` row could store no share, which is a state
-- the enum cannot represent and `from_row` would have to panic on.
CREATE TABLE fund (
  id           INTEGER PRIMARY KEY,
  name         TEXT    NOT NULL,
  ord          INTEGER NOT NULL,
  kind         TEXT    NOT NULL CHECK (kind IN ('age_over_30', 'remainder_share')),
  share_bp     INTEGER,
  actual_cents INTEGER NOT NULL DEFAULT 0,
  CHECK ((kind = 'age_over_30'     AND share_bp IS NULL)
      OR (kind = 'remainder_share' AND share_bp IS NOT NULL))
);

CREATE TABLE setting (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
