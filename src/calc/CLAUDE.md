# calc — the spreadsheet's formulas

Every function here reproduces something the workbook computes: a named lambda, or a block of the
`Planning` sheet. The workbook is the oracle, but no figure off it may be committed here, so each
`mod tests` carries *invented* inputs and the results recomputed from them by hand. Those tests
record **what** the answer is. This file records the derivations they don't show — why a formula
has the shape it has, and which of its edges are load-bearing.

No database, no I/O. `plan.rs` feeds these from `db`; `tui/planning.rs` renders the result.

## The lambdas

| Workbook | Here |
|---|---|
| `Tax(price)` | `tax::tax` |
| `Biweekly(monthly)` | `paycheck::biweekly` |
| `PerPaycheck(cur, goal, by)` | `paycheck::per_paycheck` |

**`Tax`'s threshold is on the taxed value, not the price.** `IFS(v<500, 1, TRUE, 5)` tests `v`, the
price already grossed up by the rate — so a $475 item at 6.25% is $504.69 and rounds to the next
$5 ($505), not the next dollar. Reading the threshold off the price instead would put it at $501,
a dollar out, and only for items in the narrow band that crosses $500 under tax. The whole
computation stays in basis-point-scaled integers; nothing becomes a float on the way through.

**`PerPaycheck`'s blanks are meaningful.** The sheet shows `""` for an undated goal and for one
already at or past its target, and `None` here means exactly those two cases — not "zero". The
Savings screen hides the column for them rather than printing `$0`, because a dated goal genuinely
needing nothing this paycheck and an undated goal that never had a per-paycheck figure are
different states.

**A past-due goal clamps to one paycheck.** `MAX(1, CEILING((by − today) / period, 1))` — once the
date has passed, the remaining periods go zero or negative, and without the clamp the division
either blows up or hands back a negative set-aside. Clamped, the full remainder comes due now,
which is the honest answer.

**`per_paycheck_over_years` has no lambda behind it.** The recurring-goal block carries a month of
the year and a cadence and no dates, so there is no runway for `PerPaycheck` to divide: the runway
*is* the cadence — one round's cost spread over every paycheck before it comes round again. It is
the Recurring Goals title's figure and nothing else's, so no money moves on it. `years` comes off a
`Cadence` and is never zero; `periods_per_year` is the owner's setting and is clamped the way
`Biweekly` clamps it.

## Rounding direction is not incidental

Two directions, applied by role:

- **Ceiling** wherever a *requirement* is computed — `tax`, `biweekly`, `per_paycheck`,
  `per_paycheck_over_years`. Round up, so
  the figure covers the thing it is sizing. Under-rounding a set-aside means missing the goal date.
- **Floor** (`Cents::floor_to_dollar`) on every *transfer instruction* in `planning::compute`. You
  move whole dollars, and never more than you actually have.
- **Truncation toward zero** on every *display* figure — `calc::fund`'s target, actual and delta
  percentages. Neither a requirement nor an instruction to move money: nothing is sized by these and
  nothing is transferred on them, so they take the direction that keeps a percentage from claiming a
  hundredth of a point it does not have.

Those two directions are precisely why the waterfall needs a plug: the parts are rounded one way,
the total another, and something has to absorb the difference.

`div_ceil` is in `mod.rs` rather than inlined because getting it right is fiddly: `(a + b − 1) / b`
under-reports for negative dividends (Rust's `/` truncates toward zero) and can overflow near
`i64::MAX`. It takes the `div_euclid`/`rem_euclid` route instead, and returns `Result` rather than
asserting — see the last section.

## The Planning waterfall, in order

`planning::compute` reproduces `Planning!C1:G41`. The order is the derivation:

```
excess_actual   = max(0, checking@adhoc − target − buffer)
excess_used     = pinned excess, else excess_actual floored to the dollar

− housing_biweekly       Biweekly() over the bill table's Housing rows
− other_bills_biweekly   Biweekly() over the rest
= remaining_excess       (clamped at zero)

  bill_payments = min(cap, pct × remaining_excess)
  mom_and_dad   = min(max(remaining_excess − bill_payments, 0), annual / periods)
= remainder

  remainder ≤ goals_floor  → all of it to Goals
  otherwise                → future housing %, retirement %, investment %,
                             each capped by what the ones above it left,
                             and Goals takes whatever percentage is left

then, against the excess itself:

  current_housing = min(housing_biweekly, budget)
  bills           = min(other_bills_biweekly + bill_payments,
                        budget − current_housing)
  goals           = max(0, excess_used − everything above)
```

`budget` is `excess_used` floored to the dollar. The two caps are what hold
`lines.total() <= excess_used`; see below for why those two lines and no others.

**`checking@adhoc` is the Overview's Paycheck-Eve column**, and that date is the first paycheck eve
strictly *after* today. So on the eve itself it has already rolled to the eve of the paycheck after,
and the excess is quoted at a balance that counts a paycheck which has not landed yet -- the step
the excess takes each cycle falls on the eve rather than on payday. That is deliberate and it is not
a second decision: the waterfall quotes the day the Overview shows, and a Planning screen holding
back a day the Overview had already moved past is the disagreement `plan::compute_from_db` takes the
date as an argument to prevent. A payday worked the evening before is therefore priced on the money
it is about to receive, which is what the owner is planning against; where it should not be, the
scrub moves it and `p` pins the figure shown.

Why this order: the biweekly bills are obligations, so they come out before anything proportional —
splitting first and paying bills from a share would let a good month fund investments while the
mortgage went short. `bill_payments` is `min(cap, share)` rather than a flat share so a large excess
does not pour into bill payments without limit. `mom_and_dad` is a fixed annual commitment divided
per period, but floored by what is actually left after bills, so it cannot go negative in a thin
month.

The split percentages are user-editable, and the three are bounded **as a set** as well as one at a
time: each inside `0..=100` on its own, and still wrong between them. Both writers hold both bounds
— at the form, `tui::planning::parse_percent` refuses a percentage outside the range and
`write_split` refuses the set it would join; at the import, `plan::check_splits` refuses either,
since `import::cell::as_percent` reads whatever the sheet carries and a `150 / -60 / 5` totals 95.
A share below zero is the one the set rule cannot catch on its own: `Percent::of` does not clamp,
so it reaches a line as a negative allocation, and nothing downstream reads a line's sign.

Goals' share is `Percent::ONE_HUNDRED.saturating_sub(sum)`, which now only ever saturates on a
database written before either rule existed.

### `Excess (Fixed)` became a pin

The sheet holds a hand-typed snapshot of `Excess (Actual)`. The need behind it is real and worth
understanding before touching `excess_used`:

Transfers land about three days after each paycheck, which puts them *before* the ad-hoc projection
date — the day before the *next* paycheck. So the moment you enter the first leg, `checking@adhoc`
drops by that leg's amount and `Excess (Actual)` collapses, while the remaining legs are still
unentered. The waterfall would move underneath you mid-payday.

Pressing `p` on the Planning screen snapshots the floored actual into `setting`; the waterfall runs
off the pin until `P` clears it. `e` on the `Excess (Used)` row writes the same key by hand — the
sheet's cell restored, for the payday whose excess the owner knows better than the balance does — in
whole dollars and never below zero, since `excess_used` is a whole-dollar figure in both modes and a
negative one leaves every line at nothing while claiming the excess was spent. Both bounds are held
at both writers: `tui::planning::parse_pinned_excess` on the text typed into the row, and
`plan::check_pinned_excess` on the figure an import writes. The screen shows drift since the pin, so
a stale pin is visible rather than silent, and `p` pressed again re-pins at the current figure — the
answer to a stale pin is a fresh one, not a cleared one. Unpinned, `excess_used` floors the live
actual — which is why `lines.total() <= excess_used` is answered the same way in both modes.

### The transfers never total more than the excess

**`lines.total() <= excess_used`, always**, and equal to it for any excess the app can produce.
That is the whole of what the caps below are for, and it is the property the tests pin rather than
any single figure.

Two lines could once break it, and both are *fixed* biweekly figures that do not scale with the
excess: `current_housing` (`Mortgage + HOA`) and `bills` (the rest of the monthly block, plus a
bill-payment share that is already nothing by then). Everything under them is a share of
`remaining_excess`, which is clamped at zero, so none of it can claim what is not there. A payday
too small for those two therefore used to write transfers totalling more than the account had to
give them.

So each is capped by what is left of the excess when it is reached, and **housing is paid first**:
the waterfall is an ordered priority list and `Mortgage + HOA` is the payment least able to wait, so
`Bills` is what takes the cut. `Bills` is capped against the budget less the *floored* housing, so
no cents leak between the two.

The three discretionary splits are clamped the same way, each by what the ones above it left. That
one is a backstop rather than a rule: `tui::planning::write_split` refuses a combination over 100%
at the form and `plan::check_splits` refuses one at the import, so it binds only on a database
written before those existed. It is therefore **silent** — no line reports it — where the bills cap
reports itself.

### Goals is a plug, and what it absorbs is the flooring

`lines.goals` is not computed from a percentage. It is `excess_used` minus every other floored line,
so it absorbs all the flooring and the totals reconcile exactly.

It still clamps at zero, but the clamp is now a formality: every claim above it is capped by what
the excess had left, so `claimed` never passes `excess_used` in the first place. The clamp stays
because a negative allocation to a savings goal is not a thing whatever the arithmetic says.

There is no checksum. It would report the one condition the caps prevent, and a figure that is
provably zero for every input is not a check — substitute the plug's definition into `Lines::total`
and every term cancels. What states the gap instead is `Plan::shortfall`: one `Cents` per line, what
that line lost to the cap above it. The screen and the report each draw it as a `Δ` in the cut
line's own extra cell, and foot the transfers block with its whole total — which is what states it
on the payday where every line is zero and there is no line to draw a `Δ` beside.

### The gates, in priority order

Emergency → Roth, each firing on `remaining_* > 0`. `plan.rs` resolves each remainder from goal
ids stored in `setting` under the keys `gate::Gate` owns — never by name; see the root `CLAUDE.md`
on why goal names cannot be keys.

- **Emergency shuts off everything else.** Roth, retirement and investment go to zero, and the
  future housing, retirement, and investment shares pile into `lines.emergency_fund` — summed
  first, floored once, so the separate floors don't leak a dollar or two out of the total.
- **Roth splits the retirement share.** `lines.roth` takes up to what the Roth still needs;
  `lines.retirement` gets the rest.

Each gate has its own test, because a gate with no exercising test is a branch that silently rots,
and both read false in the ordinary case.

### Future Housing is one destination, floored once

`future_housing` is the whole `future_housing_pct` share of the remainder, under one line —
`lines.future_housing`. Which account it resolves to, a down-payment goal or mortgage principal, is
a question for the layer above `calc`, which is why the share is not split here: splitting it into a
goal contribution and a mortgage overflow, the way the sheet's `D35`/`D41` do, floors twice and can
leak a dollar the single merged line does not.

## The fund allocation, in order

`fund::compute` reproduces `Planning!J2:L4`:

```
age_target  = max(0, age − BONDS_START_AGE) × 100 bp     unknown when no birth date is on record
remainder   = max(0, 10,000 − Σ age targets)              an unknown age claims nothing
target      = age_target, or remainder × share / 10,000
actual      = value × 10,000 / Σ values                   zero for every row when the total is zero
delta       = max(0, target − actual)
```

`furthest_down` is the index of the largest positive delta — the first on a tie, and `None` when
every row is at or above target.

Shares that do not add to 1.0 are computed as given, never refused: adding the first share row
always leaves the shares short, so a refusal would block ordinary entry. The screen carries a
`Total` row instead, where a mis-sum reads as `44.80` rather than `100.00`.

An unknown age is a third state beside "at target" and "below it", and it is neither an error nor a
zero: the row has no target, so it has no delta, and it is left out of the target sum rather than
counted as nothing.

## `pro_rata` splits interest by largest remainder

Shares must be whole dollars *and* sum exactly to the total. Those conflict, so it uses the
largest-remainder (Hamilton) method: floor every share, then hand out the leftover dollars one at a
time to the largest fractional remainders.

Dumping the whole leftover on the single largest share — the way `Planning!D32` absorbs its plug —
is wrong here. When several buckets round up at once the correction can exceed a share: `$2.00`
across four equal buckets produces `$1, $1, $1, −$1`. The plug works in Planning because one
designated line exists to absorb it. Here every share is a real allocation to a real goal.

Sub-dollar residue (interest postings are not whole dollars) rides with the largest fractional
remainder. A negative total is refused rather than clamped: the method is derived for a non-negative
total, and a sign flip means the caller picked up a withdrawal rather than an interest posting.

## Settings reach divisors

`periods_per_year` (`Constants!G2`) is user-editable — on the Planning screen as well as by import —
and lands in denominators. **The clamp lives with the divide**: `biweekly`, `period_days`,
`per_paycheck_over_years` and `planning::compute` each `.max(1)` their own denominator, so a nonsense
value cannot take down a whole screen and a caller has nothing left to clamp. Re-clamping in front of
one of them is a protection that cannot fire, and it reads as though the one behind it were not
there — `tui::planning::parse_percent`'s neighbour `parse_periods` is the other half, refusing the
value as it is typed so the clamps here are only ever the backstop for a database that already holds
one. `div_ceil` returns an error for a non-positive divisor in *every* build, release included: a
`debug_assert!` would compile out of the one build where dividing by zero actually matters. Don't
reintroduce a bare divide.

**The days between two paydays are `period_days` of that same count, never a setting of their
own.** The sheet carries both (`G2` and `H2`) and the two can disagree; one setting has nothing to
reconcile, and it means `per_paycheck`, which counts a deadline's runway in days, and `biweekly`
and `per_paycheck_over_years`, which spread a cost over the count, cannot come to describe
different pay cadences. `period_days` divides `WEEKS_PER_YEAR * DAYS_PER_WEEK` by the count and
clamps at both ends, so it is also where the clamp for `per_paycheck`'s divide now lives. A year of
whole weeks rather than the calendar's 365 days, because a pay cadence counts in weeks: `364`
divides exactly by every whole-week cadence, so the floor bites only on the semi-monthly and
monthly counts, whose paydays are not a fixed number of days apart to begin with.
