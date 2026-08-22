# Taxed goals: storing the base, deriving the target

A savings goal for a $1,234 item is a goal to save $1,315, because the item costs $1,315 at the
register. Today the owner types the taxed figure in by hand, or lets the recurring-goal picker run
`calc::tax` once at creation and store what came out. Either way the base is lost the moment it is
saved, and nothing on any screen can say which of the two a stored figure is.

This adds `goal.taxed`. A taxed goal stores the **base** and derives its **target** on every read,
the way `fund` stores an age rule and derives a percentage. The picker stops applying tax at
creation and hands the flag across instead, so a recurring entry marked taxed produces a goal
marked taxed.

## The decision this rests on

**The derived figure is the target everywhere, not a decoration.** A taxed goal's shortfall, its
percentage complete, its `$/Pay`, whether it counts as still short for the payday plug, and whether
it draws as overdue are all computed against the taxed figure. The stored number is the base those
come from, and the only places it is shown as itself are the two forms that edit it.

This is what makes the change worth a schema arm rather than a render tweak: a goal that funds to
its base comes up short at the register, which is the bug the whole feature exists to prevent.

## Schema

One arm, version 4, two statements:

```sql
ALTER TABLE goal RENAME COLUMN goal_cents TO base_cents;
ALTER TABLE goal ADD COLUMN taxed INTEGER NOT NULL DEFAULT 0
```

`data: None`. Nothing to move, and nothing that *could* be moved: every existing goal's stored
figure already is its target — imported ones came off a sheet whose goal column holds whatever the
owner put there, and picker-created ones had `calc::tax` applied before the insert. `taxed = 0` is
exactly the reading those rows want. There is deliberately **no back-fill** from
`recurring_goal.taxed` through `goal.recurring_goal_id`: those goals hold a taxed figure already,
so flagging them would tax it twice, and the lambda ceilings, so the base cannot be recovered by
inverting it.

`NOT NULL DEFAULT 0` rather than nullable, for the reason `favorite` is: there is no third state
between taxed and not, and every existing row is already in the second one.

**The rename is the point of doing this as one arm.** For a taxed goal the stored number stops
being the goal and becomes the base it is derived from, which is a change of meaning that no
`CHECK` can catch. Renaming the column and the Rust field makes the compiler visit all ~65 readers,
so the five that must change are found rather than remembered. `base_cents` is also the word the
Recurring Goals screen already uses for the same figure.

## The `goal` policy module

New `src/goal.rs`, the same shape as `src/fund.rs` — "reads the table and the rate out of `db`,
feeds `calc`". `db::goal` stays queries, `calc::tax` stays pure, and the rate is read in one place.

```rust
/// A goal, its balance, and the target it is funded to.
pub struct Funding {
    pub goal: db::goal::Goal,   // carries base_cents and taxed
    pub current: Cents,
    pub target: Cents,
}

pub fn target(goal: &Goal, rate: Option<BasisPoints>) -> Result<Cents>;
pub fn all_with_balances(db: &Db) -> Result<Vec<Funding>>;
pub fn list_with_balances(db: &Db, container: AccountId) -> Result<Vec<Funding>>;
pub fn shortfall(db: &Db, goal_id: GoalId) -> Result<Cents>;
```

`target` is `base_cents` for an untaxed goal and `calc::tax(base_cents, rate)` for a taxed one. The
rate is read once per call to the plural functions rather than once per goal.

**A taxed goal with no rate on record is a loud error naming `key::TAX_RATE`, not a silent
fallback to the base.** An unset key normally means a feature is off, but here the flag on the row
says tax is wanted, and quietly targeting the base would move the Planning waterfall's plug on the
strength of a missing setting — the same reasoning that makes a dangling gate key an error. The
state is hard to reach anyway: the rate arrives with `Constants`, and both places that write a
taxed goal — the goal form and `App::commit_picker` — refuse to without one.

`db::goal::shortfall` **moves** here rather than being wrapped: it is a target reader, it lives in
`db` where the rate cannot be reached, and leaving a second one behind would let `plan::remaining`
go on gating the waterfall against a base. `db::goal` keeps `balance`, which is a pure sum.

## The readers that change

| Reader | Was | Becomes |
|---|---|---|
| `plan::remaining` (Planning gates) | `db::goal::shortfall` | `goal::shortfall` |
| `transfer::shares_of` (the plug's set) | `g.current < g.goal.goal_cents` | `f.current < f.target` |
| `tui::paycheck_ask` | `&GoalWithBalance`, `goal.goal_cents` | `&Funding`, `f.target` |
| `tui::savings::set_goals` | `percent`/`expired`/`goal` off `goal_cents` | all three off `target` |
| `tui::app::new_worksheet` | `db::goal::list_with_balances` | `goal::list_with_balances` |

Readers that want names and ids rather than targets keep using `db::goal` untouched:
`transfer::open_goals` and `unclaimed_goals`, the destination picker's offers, the close-out's
sibling list, and the interest worksheet's line list.

## The recurring-goal handoff

`App::commit_picker` stops calling `calc::tax`. It writes `base_cents: entry.base_cents, taxed:
entry.taxed` and lets the derivation do the rest. A goal created from a taxed entry is then
indistinguishable from one the owner marked taxed by hand, which is the point: the flag survives
into the goal instead of being spent at creation.

`commit_picker` is also the second writer of a taxed goal, alongside the goal form, so it goes on
reading `key::TAX_RATE` — not to derive anything, but to refuse the whole commit with
`goal::NO_TAX_RATE` when any chosen entry is taxed and no rate is on record. Without that check the
recurring-goal form has no rate field of its own to ask through — its own comment says an entry
writes no goal — so the picker is where a taxed entry's rate is actually wanted, before there is a
goal for the read side to call corrupt.

## The two forms

Both forms hold a base and show what it comes to, in the same words:

```
        Name  Foo                              Name  Foo
      Target  1,234.00  (1,315 w/ tax)         Base  1,234.00  (1,315 w/ tax)
   Goal Date  2026-10-01                      Month  October
       Taxed  yes                             Taxed  yes
    Interest  yes                           Cadence  annual
```

- **Goal form** (`src/tui/goal_form.rs`): `GoalField::Tax` becomes `GoalField::Taxed`, labelled
  `Taxed` to match the Recurring Goals form — it is a property of the goal now, not an instruction
  to rewrite a figure. `commit` stops applying tax and puts the flag on `GoalEdit`; `GoalEdit` and
  `NewGoal` each gain `taxed`. `tax_note` stays exactly as built. `GoalForm::new` prefills the flag
  from the goal it opens on, which is what makes the field an *edit* rather than a one-shot.
  **The commit goes on refusing a taxed goal when no rate is on record**, with `NO_TAX_RATE`, even
  though it no longer needs the rate to compute anything: letting it through would write precisely
  the row the read side calls corrupt, and the form is the one place that can ask for the rate
  before there is a goal to be broken by its absence.
- **Recurring goal form** (`src/tui/recurring_goal.rs`): its `Base` field gains the same note, so
  the two forms answer the same question the same way. This is the only change to that screen; its
  `Base` and `Taxed` columns already say what they say.

**The Target field's label** stays `Target` rather than becoming `Base`. `Base` would match the
other form exactly, but this field is the goal's target for every goal that is not taxed, which is
most of them, and the note is what disambiguates the ones that are. Worth a second opinion.

## What does not change

- **The Savings screen.** The `Goal` column goes on showing one figure and that figure is the
  target, which is what it already showed. Nothing marks a taxed goal in the list; the base is a
  keystroke away on the form.
- **The import.** The workbook's `Savings!C` holds whatever the owner typed, tax included where
  they applied it, and the sheet carries no flag beside it. Imported goals arrive `taxed = 0`
  holding their target, which is true. `recurring_goal.taxed` likewise goes on arriving `false`
  from `O:Q` and being set on the Recurring Goals screen.
- **`calc::tax`.** Untouched. It is already the workbook's lambda and already has the workbook's
  own cells as its test cases.
- **`--replace`.** `goal` is still an imported table and taxed goals are cleared with the rest.

## Testing

Unit tests, against `db::open_in_memory`, in the module under test:

- `src/db/migration.rs` — a database at version 3 comes out at 4 with both columns, and an
  existing goal's figure survives the rename unchanged with `taxed = 0`.
- `src/goal.rs` — an untaxed goal's target is its base; a taxed goal's is the lambda's figure; a
  taxed goal with no rate on record errors naming the key; an untaxed goal on a rate-less database
  resolves fine.
- `src/transfer.rs` — a taxed goal sitting at its base is still short, so it is in the plug's set
  and is offered a share. This is the test that would fail if any reader kept using the base.
- `src/plan.rs` — a gate over a taxed goal is not satisfied until the taxed figure is funded.
- `src/tui/savings.rs` — `percent` and `expired` for a taxed goal are computed against the target.
- `src/tui/goal_form.rs` — the flag round-trips through `commit`; the form opens on a taxed goal
  with the selector already on; the note is unchanged from what it draws today.
- `src/tui/app.rs` — the picker creates a goal from a taxed entry carrying the base and the flag,
  not the taxed figure and no flag; `$/Pay` for that goal asks against the taxed target.
- `src/tui/recurring_goal.rs` — the `Base` field's note.

## Documentation

- `CLAUDE.md` (root) — the architecture table gains `src/goal.rs`, beside `src/fund.rs` and for
  the same reason. A new invariant: a goal's target is derived, never stored, and the base is what
  the table holds.
- `src/tui/CLAUDE.md` — the `Add Tax` invariant written for the previous design is replaced: the
  flag is stored, the target is derived, and the note is on both forms.
- `src/calc/CLAUDE.md` — nothing. `tax` is unchanged.

## Starting point

The branch already carries an unreleased first attempt at this feature: `Add Tax` as a transient
form field that rewrote the target at commit. The parts that survive are the selector, the note,
`GoalForm`'s rate, and `NO_TAX_RATE`; what changes is that `commit` stores the flag rather than
spending it. That work is a starting point for the form, not something to unpick first.
