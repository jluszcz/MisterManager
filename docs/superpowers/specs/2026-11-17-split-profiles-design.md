# Split Profiles — design

## Intent

The Planning screen's Split block — Future Housing, Retirement and Investment
percentages, with Goals as the plug — changes with the owner's year: lower
retirement/investment shares while a 401k is being contributed to, higher once
it is maxed. Today that means retyping three percentages and remembering what
they were. A *split profile* is a named set of the three, and switching profiles
is one keystroke.

## Goals

- Named profiles, each holding the three split percentages.
- `←`/`→` on the Split block switches the active profile, and the switch is
  live: the waterfall, `t`, and the report all read the active profile.
- `a`/`e`/`d` are contextual: on the Split block they add, edit and delete
  profiles; elsewhere they keep their current meanings.
- `e` is the one edit key on Planning; `E` is dropped.

## Non-goals

- Date- or event-triggered switching (e.g. "switch when the 401k is maxed").
  Switching is manual.
- Profiles for anything outside the three split percentages (Bill Payments %,
  Cap, Goals Floor stay single constants).
- Reordering profiles.

## Anti-goals

- A `--replace` losing the profiles. They are the owner's, not the workbook's.
- A profile set summing over 100%: every existing split rule holds per profile.

## Approaches

1. **A `split_profile` table, preserved across `--replace`, with an `active`
   flag** *(chosen)*. Profiles are rows the owner creates, like accounts, so they
   get a table and `PRESERVED_TABLES`. Which one is active lives on the row, not
   in `setting`, because `setting` is cleared by `--replace`.
2. **Profiles serialized into a `setting` value.** No migration, but `setting`
   is in `IMPORTED_TABLES`, so every `--replace` would wipe them — the anti-goal.
3. **Per-profile setting keys.** Settings are `Key<T>` constants, so a
   variable number of profiles cannot be keyed that way.

## Design

### Data

- Migration arm adds `split_profile (id, name TEXT NOT NULL, future_housing_bp,
  retirement_bp, investment_bp, active INTEGER NOT NULL DEFAULT 0)`, with a
  `CHECK` per share in `0..=10000` and a partial unique index allowing at most
  one `active = 1`. A `SplitProfileId` id type in `src/db/id.rs`.
- The migration seeds one active profile from the three existing `SPLIT_*`
  settings (or the `plan.rs` defaults when unset), named `Default`; the three
  setting keys are then no longer read.
- `split_profile` joins `PRESERVED_TABLES`.
- `db::split_profile` module, `select_split_profile!` macro, following the
  existing query-module shape. `set_active` clears every other row in one
  transaction (the `recurring_txn::set_paycheck` construction). Delete refuses
  the last profile; deleting the active one activates a neighbour.
- `plan::settings_from_db` reads the active profile's shares. No active
  profile with rows present is a corrupt database and errors loudly.

### Import

`import` writes the sheet's three percentages (`Planning!J*` cells it reads
today) into the active profile, creating a `Default` one on an empty database.
`plan::check_splits` still validates the imported set.

### Planning screen

- The Split heading row names the profile and shows that others exist:
  `Split  ◀ 401k ▶` (arrows only when there is more than one profile; masked
  name under `--demo`).
- With the cursor on the Split heading or any split row (Future Housing,
  Retirement, Investment, Goals):
  - `←`/`→` activates the previous/next profile (wrapping), reloads, status
    line names it.
  - `e` on a percentage row edits that share in the active profile
    (`write_split`'s set-bound refusal unchanged); `e` on the heading renames
    the profile.
  - `a` asks a name and adds a profile copying the active one's shares, and
    activates it.
  - `d` confirms, then deletes the active profile; refused when it is the last.
- Elsewhere, `a`/`d` keep adding/deleting bills.
- The Split heading becomes a cursor-reachable row (today headings are
  skipped) so the rename has somewhere to live.

### `e` / `E`

`e` on a bill row opens the full `BillForm` (label, amount, category); the
one-line amount box for bills goes away and `E` is unbound. Help entries and
`src/tui/AGENTS.md` key tables updated to match.

### Report

The Planning tab's Split heading carries the active profile's name, read
through `plan_rows`, so both mediums show the same heading.

## Left to the builder

- Exact Target/Editable variants for the heading and profile rows.
- Whether `Target::FutureHousingPct` etc. keep their names or take a profile id.
- Help text wording; test names per the repo's sentence convention.
