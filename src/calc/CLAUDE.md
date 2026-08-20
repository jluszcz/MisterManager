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

## Rounding direction is not incidental

Two directions, applied by role:

- **Ceiling** wherever a *requirement* is computed — `tax`, `biweekly`, `per_paycheck`. Round up, so
  the figure covers the thing it is sizing. Under-rounding a set-aside means missing the goal date.
- **Floor** (`Cents::floor_to_dollar`) on every *transfer instruction* in `planning::compute`. You
  move whole dollars, and never more than you actually have.
- **Truncation toward zero** on every *display* figure — `calc::fund`'s target, actual and delta
  percentages. Neither a requirement nor an instruction to move money: nothing is sized by these and
  nothing is transferred on them, so they take the direction that keeps a percentage from claiming a
  hundredth of a point it does not have.

Those two directions are precisely why the waterfall needs a plug and a checksum: the parts are
rounded one way, the total another, and something has to absorb the difference.

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
                             and Goals takes whatever percentage is left
```

Why this order: the biweekly bills are obligations, so they come out before anything proportional —
splitting first and paying bills from a share would let a good month fund investments while the
mortgage went short. `bill_payments` is `min(cap, share)` rather than a flat share so a large excess
does not pour into bill payments without limit. `mom_and_dad` is a fixed annual commitment divided
per period, but floored by what is actually left after bills, so it cannot go negative in a thin
month.

The split percentages are user-editable and can be set to more than 100 between them. Goals' share
is `Percent::ONE_HUNDRED.saturating_sub(sum)`, so a misconfiguration zeroes Goals rather than
allocating it a negative amount.

### `Excess (Fixed)` became a pin

The sheet holds a hand-typed snapshot of `Excess (Actual)`. The need behind it is real and worth
understanding before touching `excess_used`:

Transfers land about three days after each paycheck, which puts them *before* the ad-hoc projection
date — the day before the *next* paycheck. So the moment you enter the first leg, `checking@adhoc`
drops by that leg's amount and `Excess (Actual)` collapses, while the remaining legs are still
unentered. The waterfall would move underneath you mid-payday.

Pressing `p` on the Planning screen snapshots the floored actual into `setting`; the waterfall runs
off the pin until it is cleared. The screen shows drift since the pin, so a stale pin is visible
rather than silent. Unpinned, `excess_used` floors the live actual — which is why the checksum holds
in both modes.

### Goals is a plug, and the clamp is what makes the checksum real

`lines.goals` is not computed from a percentage. It is `excess_used` minus every other floored line,
so it absorbs all the flooring and the totals reconcile exactly.

It clamps at zero. Unclamped it goes negative whenever the excess cannot cover the fixed biweekly
bills, which would mean reporting a negative allocation to a savings goal — not a thing. Clamped,
the shortfall surfaces in `checksum` instead, negative by exactly the amount short.

That clamp is also the only reason `checksum` tests anything. Substitute the unclamped plug's
definition into `Lines::total` and every term cancels: the checksum is algebraically zero for *any*
input and can never detect a mistake. With the clamp it is zero when the plan balances and negative
by the shortfall when it doesn't.

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

`periods_per_year` (`Constants!G2`) and `period_days` (`Constants!H2`) are user-editable and land in
denominators. Call sites clamp with `.max(1)` where a nonsense value should not take down a whole
screen; `div_ceil` returns an error for a non-positive divisor in *every* build, release included.
A `debug_assert!` would compile out of the one build where dividing by zero actually matters. Don't
reintroduce a bare divide.
