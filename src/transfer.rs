//! Turning a computed plan into the transfers that execute it.
//!
//! Policy over `db::txn`, the way [`crate::recurring_txn`] is policy over its
//! own table: it resolves every Planning line to an account or to none,
//! groups by that answer, and sums. Nothing here decides *amounts* -- `calc`
//! does that -- and nothing in `calc` decides destinations.

use crate::calc::planning::Lines;
use crate::db::account::{self, AccountColor};
use crate::db::goal::Goal;
use crate::db::setting::Key;
use crate::db::txn::{self, NewTxn};
use crate::db::{AccountId, Db, GoalId, goal, setting};
use crate::goal as goal_engine;
use crate::money::Cents;
use crate::plan_line::{Destination, Line};
use crate::reading::Reading;
use anyhow::{Context, Result, anyhow, bail, ensure};
use chrono::NaiveDate;

/// One row a payday writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Row {
    /// Money moving to an account the database tracks, as one two-leg
    /// transfer carrying every line that lands there.
    Transfer {
        to: AccountId,
        name: String,
        /// The color the owner picked for that account, if any -- carried
        /// beside its name so the Planning screen tints the row it heads the
        /// same shade the Account column carries everywhere else. `plan` has
        /// the account row in hand and the screen does not.
        color: Option<AccountColor>,
        cents: Cents,
        lines: Vec<(Line, Cents)>,
    },
    /// Money leaving the tracked system. One row per line, not one lump:
    /// Retirement and Investment go to different places in the real world,
    /// and reading `2,070 Retirement` off the ledger is worth more than one
    /// row reading `4,140`.
    Withdrawal { line: Line, cents: Cents },
}

impl Row {
    pub fn cents(&self) -> Cents {
        match self {
            Row::Transfer { cents, .. } => *cents,
            Row::Withdrawal { cents, .. } => *cents,
        }
    }
}

/// The goals no line claims.
///
/// Not [`spread_goals`], which narrows this to the ones still short: a goal
/// sitting at its target takes none of the plug and is still a perfectly good
/// destination for a line.
///
/// Open goals only, and in the order the Savings screen lists them.
pub fn unclaimed_goals(db: &Db) -> Result<Vec<Goal>> {
    let claimed = claimed_goals(db, Reading::Strict)?;
    Ok(open_goals(db)?
        .into_iter()
        .filter(|g| !claimed.contains(&g.id))
        .collect())
}

/// Every open goal, in the order the Savings screen lists them.
fn open_goals(db: &Db) -> Result<Vec<Goal>> {
    Ok(goal::all_with_balances(db)?
        .into_iter()
        .map(|g| g.goal)
        .collect())
}

/// The goals a payday's plug is spread over.
///
/// Not every unclaimed goal: a goal sitting at its target needs nothing, so
/// it is offered nothing. That matters twice over, because the same set
/// decides *where* the plug lands -- a met goal in a second container used to
/// make the plug ambiguous over money it could never have received, which is
/// exactly what a line switched to a withdrawal leaves behind.
///
/// *How much* each one gets is not decided here: `savings::paycheck_ask` prices
/// them and [`crate::calc::fit`] divides the plug between them. This is only
/// the set.
///
/// When every unclaimed goal is met the set is all of them rather than none,
/// so the plug still has a container to land in. Their asks are all zero, so
/// the whole plug ends up unallocated -- the right answer when everything is
/// already funded.
pub fn spread_goals(db: &Db, reading: Reading) -> Result<Vec<Goal>> {
    Ok(cloned(shares_of(&unclaimed_with_balances(db, reading)?)))
}

/// The plug's set, with what each of its goals asks of this paycheck.
///
/// The set and its pricing off one read, because they answer one question:
/// `calc::fit` divides the plug by these asks, and the Planning screen
/// reports whether the Goals line covers their sum. Two callers deriving
/// the pair separately is two chances to disagree about which goals the plug
/// is even for.
///
/// A goal with nothing to ask -- undated, or already at its target -- comes
/// back at zero rather than dropping out: it is in the set, and `fit` reads
/// a zero ask as "hand this one nothing".
pub fn spread_asks(db: &Db, today: NaiveDate, period_days: i64) -> Result<Vec<(Goal, Cents)>> {
    let unclaimed = unclaimed_with_balances(db, Reading::Strict)?;
    let mut priced = Vec::new();
    for funding in shares_of(&unclaimed) {
        let ask = crate::savings::paycheck_ask(funding, today, period_days)?;
        priced.push((funding.goal.clone(), ask.unwrap_or(Cents::ZERO)));
    }
    Ok(priced)
}

/// What a plan with nothing in any line reports.
///
/// Named rather than written twice, the same reason `goal::NO_TAX_RATE` is:
/// the Planning screen has to tell this apart from a plan that *failed* to
/// resolve. Every line zero is not a failure -- there is nothing to resolve,
/// no goal in the wrong container and nothing `Enter` could explain -- so the
/// screen states it plainly instead of under the `unresolved` label it gives
/// a plan that cannot run.
pub const NOTHING_TO_TRANSFER: &str = "nothing to transfer";

/// How far the plug falls short of what its goals asked of this paycheck, or
/// `None` when it covers them.
///
/// The plug and not a [`Line`]: only [`Line::Goals`] is ever divided this
/// way, and its figure is `lines.goals` whether or not `plan` found a
/// transfer row to carry it. A payday whose plug is nothing has no such row
/// -- `plan` skips a line at zero -- and that is the payday whose goals are
/// worst served, so a gap taking the row as its input would fade out exactly
/// as the condition it reports got worse.
///
/// One rule rather than one per sink: the Planning screen draws this in a
/// table cell and the report in a `<td>`, and two of them deciding
/// separately when a payday is under-funded is two chances to disagree about
/// the one thing the owner reads either of them for.
pub fn unmet_asks(plug: Cents, asked: Cents) -> Option<Cents> {
    (asked > plug).then(|| plug - asked)
}

/// The goals themselves, for the callers that want the set and not its
/// funding.
fn cloned(funding: Vec<&goal_engine::Funding>) -> Vec<Goal> {
    funding.into_iter().map(|f| f.goal.clone()).collect()
}

/// Every open goal no line claims, with its balance and its target.
///
/// Both halves take the one [`Reading`], because a screen has two things to
/// read past and they arrive together: a setting key naming a goal that is
/// gone, and a taxed goal with no rate on record. Reading the claims one way
/// and the targets the other is not a combination any caller wants, so the
/// claim list is read here rather than handed in -- the pairing cannot be got
/// wrong at a call site that never states it.
fn unclaimed_with_balances(db: &Db, reading: Reading) -> Result<Vec<goal_engine::Funding>> {
    let claimed = claimed_goals(db, reading)?;
    Ok(unclaimed_funding(
        goal_engine::all_with_balances(db, reading)?,
        &claimed,
    ))
}

/// The filter both readings share, so "unclaimed" cannot come to mean two
/// things across them.
fn unclaimed_funding(
    all: Vec<goal_engine::Funding>,
    claimed: &[GoalId],
) -> Vec<goal_engine::Funding> {
    all.into_iter()
        .filter(|f| !claimed.contains(&f.goal.id))
        .collect()
}

/// The filter itself, over a set the claims have already been taken out of.
///
/// Split out because the claim list reaching it differs by caller: [`plan`]
/// reads claims strictly and refuses a dangling key, while [`wiring`] has to
/// report one and draw the screen anyway. Which goals the plug spreads over
/// is the same question either way, and this is the one answer to it.
///
/// Short means short of the **target**, so a taxed goal sitting at its base is
/// still in the set: what it needs is the tax.
fn shares_of(unclaimed: &[goal_engine::Funding]) -> Vec<&goal_engine::Funding> {
    let short: Vec<&goal_engine::Funding> =
        unclaimed.iter().filter(|f| f.current < f.target).collect();
    if !short.is_empty() {
        return short;
    }
    unclaimed.iter().collect()
}

/// The distinct containers the plug's goals sit in, in the order the Savings
/// screen lists them.
fn spread_containers(goals: &[Goal]) -> Vec<AccountId> {
    let mut containers: Vec<AccountId> = Vec::new();
    for g in goals {
        if !containers.contains(&g.container_account_id) {
            containers.push(g.container_account_id);
        }
    }
    containers
}

/// `a`, `a and b`, `a, b and c`.
///
/// Shared with the Planning screen's duplicate-payday warning, which lists
/// dates rather than containers: two sentences listing things in two
/// house styles is what one joiner exists to prevent.
pub fn joined(names: &[String]) -> String {
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}

/// The same sentence starting a line rather than following "Error: ".
///
/// Errors are written lowercase by convention and read that way when they
/// are quoted into a status line; the panel puts the same words at the head
/// of a paragraph, where a lowercase first letter reads as a fragment.
fn capitalized(sentence: &str) -> String {
    let mut chars = sentence.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// The one sentence the refusal and the details panel both lead with, so the
/// row and the panel cannot come to describe the same state differently.
fn ambiguous_plug(containers: &[String]) -> String {
    format!(
        "the goals no Planning line claims sit in {}, so the Goals plug has no one container to land in",
        joined(containers)
    )
}

/// The container the plug lands in, or `None` when no goal is unclaimed.
///
/// An error when the unclaimed goals span more than one container: the plug
/// is a single amount and there is no rule for dividing it. The message names
/// the containers, because the owner reading it has to find the goal in the
/// wrong one to act on it.
pub fn spread_container(db: &Db) -> Result<Option<AccountId>> {
    let containers = spread_containers(&spread_goals(db, Reading::Strict)?);
    if containers.len() > 1 {
        bail!("{}", ambiguous_plug(&container_names(db, &containers)?));
    }
    Ok(containers.first().copied())
}

fn container_names(db: &Db, ids: &[AccountId]) -> Result<Vec<String>> {
    ids.iter()
        // names the containers in a text report, not a display of an account
        .map(|id| Ok(account::get(db, *id)?.name.as_str().to_string()))
        .collect()
}

/// Where one Planning line's money lands, as the settings say today.
///
/// The read side of what [`plan`] enforces: same rules, but nothing here
/// refuses. A screen showing a corrupt database still has to draw itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Landing {
    /// The key names this goal, which sits in this container.
    Goal { goal: String, container: Container },
    /// The key names this account directly -- the two lines with no goal
    /// behind them.
    Account { account: Container },
    /// The plug, spread over the unclaimed goals of this one container.
    Spread { container: Container },
    /// The plug with nowhere single to land: unclaimed goals sit in all of
    /// these containers, and there is no rule for dividing one amount.
    Ambiguous { containers: Vec<String> },
    /// The plug with no unclaimed goal at all to spread over.
    Nowhere,
    /// The key is unset. For a destination that is a supported state and not
    /// an error: the money leaves the tracked system.
    Withdrawal,
    /// The key names a goal or account that is gone -- a corrupt database,
    /// and never to be read as a withdrawal.
    Dangling { key: String },
}

impl Landing {
    /// Whether this landing is why [`plan`] would refuse.
    ///
    /// [`Landing::Withdrawal`] is deliberately not one: unset means the money
    /// leaves the tracked system, which is how Retirement and Investment are
    /// meant to stand.
    pub fn breaks_the_plan(&self) -> bool {
        matches!(
            self,
            Landing::Ambiguous { .. } | Landing::Nowhere | Landing::Dangling { .. }
        )
    }
}

/// An account a landing names, as a screen needs it: what to call it, and
/// which account it is.
///
/// The id is here so the Planning screen can tint a destination the same
/// color the Account column carries on every other screen -- a container
/// named in one shade on Savings and another on Planning would be two
/// screens disagreeing about the same account. The color travels with it
/// rather than being looked up again, because `wiring` has the account row
/// in hand and the screen does not.
///
/// Only where *one* account is named: an ambiguous plug spans several, and
/// there is no single color for a list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Container {
    pub id: AccountId,
    pub name: String,
    pub color: Option<AccountColor>,
}

/// One line, where it lands, and what its name would suggest if it landed
/// nowhere.
#[derive(Debug, Clone)]
pub struct Wiring {
    pub line: Line,
    pub landing: Landing,
    /// The goal this line's import substring matches today, offered only when
    /// the line points at no live goal and exactly one unclaimed goal
    /// matches.
    ///
    /// **Advisory, and never a resolution.** Name matching that decides where
    /// money goes happens once, at import, because goal names are not unique;
    /// this is a prompt a human answers by pressing a key, and the id it
    /// writes is what every later read resolves by.
    pub suggestion: Option<Goal>,
}

/// Every line's destination, for the screen that shows them.
///
/// Two passes, because the plug's landing depends on which goals the other
/// lines claim -- and the claims are read tolerantly here, so one dangling
/// key is reported as itself rather than taking the whole block down.
pub fn wiring(db: &Db) -> Result<Vec<Wiring>> {
    let accounts = account::list(db)?;
    let container_of = |id: AccountId| {
        accounts.iter().find(|a| a.id == id).map(|a| Container {
            id: a.id,
            // A `String`, not a `label::Account`: the Planning screen
            // tints this through `planning::Tint` instead -- see
            // `src/tui/CLAUDE.md`'s account-color section.
            name: a.name.as_str().to_string(),
            color: a.color,
        })
    };
    let name_of = |id: AccountId| container_of(id).map(|c| c.name);

    let mut landings: Vec<(Line, Landing)> = Vec::new();
    for line in Line::ALL {
        let landing = match line.destination() {
            // Resolved in the second pass, once every claim is known.
            Destination::Spread => continue,
            Destination::Goal(key) => match setting::get(db, key)? {
                None => Landing::Withdrawal,
                Some(id) => match goal::get(db, id)? {
                    None => Landing::Dangling {
                        key: key.name().to_string(),
                    },
                    Some(goal) => match container_of(goal.container_account_id) {
                        None => Landing::Dangling {
                            key: key.name().to_string(),
                        },
                        Some(container) => Landing::Goal {
                            goal: goal.name,
                            container,
                        },
                    },
                },
            },
            Destination::Account(key) => match setting::get(db, key)? {
                None => Landing::Withdrawal,
                Some(id) => match container_of(id) {
                    None => Landing::Dangling {
                        key: key.name().to_string(),
                    },
                    Some(account) => Landing::Account { account },
                },
            },
        };
        landings.push((line, landing));
    }

    // The same tolerant reader `diagnose` uses. Accumulating a second claim
    // list inside the loop above would be a second copy of one rule, and the
    // plug's landing disagreeing with the panel's breakdown is precisely the
    // failure that costs.
    let with_balances = unclaimed_with_balances(db, Reading::Tolerant)?;
    let unclaimed: Vec<Goal> = with_balances.iter().map(|g| g.goal.clone()).collect();
    let containers = spread_containers(&cloned(shares_of(&with_balances)));
    let spread = match containers.len() {
        0 => Landing::Nowhere,
        1 => match container_of(containers[0]) {
            Some(container) => Landing::Spread { container },
            None => Landing::Nowhere,
        },
        _ => Landing::Ambiguous {
            containers: containers.iter().filter_map(|id| name_of(*id)).collect(),
        },
    };

    Line::ALL
        .iter()
        .map(|line| {
            let landing = match line.destination() {
                Destination::Spread => spread.clone(),
                _ => landings
                    .iter()
                    .find(|(l, _)| l == line)
                    .map(|(_, landing)| landing.clone())
                    .expect("every non-plug line was resolved in the first pass"),
            };
            let suggestion = match landing {
                // A line pointing at a live goal is not asking, and the plug
                // is not matched by name at all.
                Landing::Withdrawal | Landing::Dangling { .. } => {
                    suggestion_for(*line, &unclaimed).cloned()
                }
                _ => None,
            };
            Ok(Wiring {
                line: *line,
                landing,
                suggestion,
            })
        })
        .collect()
}

/// The one unclaimed goal `line`'s import substring matches, or `None` when
/// none does -- or when several do.
///
/// Several is the case that matters: "Lego" names several goals in the
/// workbook, and offering the first would be choosing between them by luck.
fn suggestion_for(line: Line, unclaimed: &[Goal]) -> Option<&Goal> {
    let substring = line.import_substring()?;
    let mut matches = unclaimed.iter().filter(|g| g.name.contains(substring));
    match (matches.next(), matches.next()) {
        (Some(goal), None) => Some(goal),
        _ => None,
    }
}

/// What one line's name would suggest, for the picker that opens on it.
pub fn suggest(db: &Db, line: Line) -> Result<Option<Goal>> {
    Ok(wiring(db)?
        .into_iter()
        .find(|w| w.line == line)
        .and_then(|w| w.suggestion))
}

/// Why [`plan`] refuses, at the length a panel can hold rather than the
/// length a table cell can.
///
/// Empty exactly when the plan resolves, which is what tells the screen there
/// is nothing to open. The refusal itself is the last resort: every case with
/// something specific to say says it, and anything else falls back to the
/// message `plan` produced rather than to silence.
pub fn diagnose(db: &Db, lines: &Lines) -> Result<Vec<String>> {
    // Not `plan`'s refusal alone. A landing that `Landing::breaks_the_plan`
    // paints red must have something to say when `Enter` asks, and two of
    // them survive a `plan` that resolves: `plan` skips a zero-amount line
    // before ever resolving its destination, while `t` goes on to refuse an
    // ambiguous plug whatever the amount. A screen that alarms and then
    // denies the alarm is worse than either signal alone.
    let refusal = plan(db, lines).err();
    let wiring = wiring(db)?;
    let mut out: Vec<String> = Vec::new();

    for w in &wiring {
        if let Landing::Dangling { key } = &w.landing {
            out.push(format!("{} points at a row that is gone.", w.line.label()));
            out.push(format!(
                "The setting {key} names a goal or account this database no longer holds. That is \
                 corruption rather than a gap: nothing moves for this line until it is pointed at \
                 something real, and it must never be read as a withdrawal."
            ));
            out.push(String::new());
        }
    }

    let plug = crate::demo::figure(Line::Goals.amount(lines));
    if let Some(w) = wiring.iter().find(|w| w.line == Line::Goals) {
        match &w.landing {
            Landing::Ambiguous { containers } => {
                out.push(format!(
                    "The Goals plug is {plug} and has nowhere single to go."
                ));
                out.push(String::new());
                out.push(format!("{}:", capitalized(&ambiguous_plug(containers))));
                out.extend(unclaimed_by_container(db)?);
                out.push(String::new());
                out.push(
                    "The plug is one amount and there is no rule for dividing it. Point a line at \
                     the goals in the smaller container, or close them, so every goal no line \
                     claims shares one container."
                        .to_string(),
                );
            }
            Landing::Nowhere => {
                out.push(format!(
                    "The Goals plug is {plug}, and every open goal is already claimed by a \
                     Planning line, so there is nothing left for it to spread over."
                ));
            }
            _ => {}
        }
    }

    // The refusal is the last resort rather than the lead: every state with
    // something specific to say has said it above, and a `plan` that
    // resolved leaves nothing to fall back to.
    if out.is_empty()
        && let Some(refusal) = refusal
    {
        out.push(format!("{refusal:#}"));
    }
    Ok(out)
}

/// One indented line per container: how many of the plug's goals sit there,
/// and which they are when few enough to name.
///
/// The plug's goals rather than every unclaimed one, because those are what
/// put the containers in disagreement -- naming a met goal here would send
/// the owner after a goal that is not in play.
fn unclaimed_by_container(db: &Db) -> Result<Vec<String>> {
    /// Past this many, the names stop being an aid and start being a list.
    const NAMED: usize = 5;

    let goals = spread_goals(db, Reading::Tolerant)?;
    let mut out = Vec::new();
    for id in spread_containers(&goals) {
        let names: Vec<&str> = goals
            .iter()
            .filter(|g| g.container_account_id == id)
            .map(|g| g.name.as_str())
            .collect();
        let count = match names.len() {
            1 => "1 goal".to_string(),
            n => format!("{n} goals"),
        };
        let listed = if names.len() <= NAMED {
            format!(" -- {}", names.join(", "))
        } else {
            String::new()
        };
        out.push(format!(
            "  {}: {count}{listed}",
            account::get(db, id)?.name.as_str()
        ));
    }
    Ok(out)
}

/// The goal a setting key names. `None` when the key is unset -- and, under
/// [`Reading::Tolerant`], when it names a goal that is gone.
///
/// The one place a `Key<GoalId>` becomes a [`Goal`], so [`destination_account`]
/// and [`claimed_goals`] cannot tell an unset key from a dangling one
/// differently. Only the dangling row bends: a failed query is an error under
/// either reading.
fn resolve_goal(db: &Db, key: Key<GoalId>, reading: Reading) -> Result<Option<Goal>> {
    let Some(id) = setting::get(db, key)? else {
        return Ok(None);
    };
    match (goal::get(db, id)?, reading) {
        (Some(goal), _) => Ok(Some(goal)),
        (None, Reading::Tolerant) => Ok(None),
        (None, Reading::Strict) => bail!("setting {key} = {id} names no goal"),
    }
}

/// Every goal a line names, so the plug is not spread over one of them and
/// funded twice.
///
/// Validates each reference the same way [`destination_account`] does:
/// [`spread_container`] is often resolved before the line whose key is
/// dangling, so a corrupt reference must surface here too, not only when
/// that line's own turn comes up.
fn claimed_goals(db: &Db, reading: Reading) -> Result<Vec<GoalId>> {
    let mut claimed = Vec::new();
    for line in Line::ALL {
        if let Destination::Goal(key) = line.destination()
            && let Some(goal) = resolve_goal(db, key, reading)?
        {
            claimed.push(goal.id);
        }
    }
    Ok(claimed)
}

/// Where one line's money goes, or `None` when it leaves the tracked system.
///
/// An unset key is `None`: the feature is off, and for a destination key off
/// means the money goes out. A key pointing at a goal or account that is gone
/// is an error naming the key -- never a silently reinterpreted withdrawal,
/// which would move real money to the wrong place.
///
/// Never called with a line whose destination is `Spread`: `plan` is this
/// function's only caller and resolves the plug itself, through
/// `spread_container`, so it can defer "nowhere to spread" rather than
/// letting it fall out as an ordinary withdrawal -- the exact conflation
/// this module exists to prevent.
fn destination_account(db: &Db, line: Line) -> Result<Option<AccountId>> {
    match line.destination() {
        Destination::Goal(key) => {
            Ok(resolve_goal(db, key, Reading::Strict)?.map(|g| g.container_account_id))
        }
        Destination::Account(key) => match setting::get(db, key)? {
            None => Ok(None),
            Some(id) => {
                account::get(db, id)
                    .with_context(|| format!("setting {key} = {id} names no account"))?;
                Ok(Some(id))
            }
        },
        Destination::Spread => unreachable!("plan resolves the plug itself"),
    }
}

/// The account every transfer leaves from.
///
/// The same account whose balance is the waterfall's `Excess (Actual)`, and
/// through the same read, so a payday cannot leave from an account the plan
/// did not count.
pub fn source(db: &Db) -> Result<AccountId> {
    Ok(account::checking(db)?.id)
}

/// Adds `cents` for `line` to the transfer already carrying money to `to`,
/// or opens a new one -- the one place two lines sharing a destination
/// become one row rather than two.
fn merge_transfer(
    transfers: &mut Vec<Row>,
    db: &Db,
    to: AccountId,
    line: Line,
    cents: Cents,
) -> Result<()> {
    if let Some(Row::Transfer {
        cents: total,
        lines,
        ..
    }) = transfers
        .iter_mut()
        .find(|r| matches!(r, Row::Transfer { to: at, .. } if *at == to))
    {
        *total += cents;
        lines.push((line, cents));
        return Ok(());
    }
    let account = account::get(db, to)?;
    transfers.push(Row::Transfer {
        to,
        // A `String`, not a `label::Account`: the Planning screen tints
        // this through `planning::Tint` instead -- see `src/tui/CLAUDE.md`'s
        // account-color section.
        name: account.name.as_str().to_string(),
        color: account.color,
        cents,
        lines: vec![(line, cents)],
    });
    Ok(())
}

/// The rows a payday writes, grouped by destination and summed.
///
/// Transfers first, in the order their lines appear in [`Line::ALL`], then
/// the withdrawals in the same order.
///
/// A zero-valued line writes no row at all, transfer or withdrawal: it moves
/// nothing, and a zero row would hide the ones that do. That includes a zero
/// `Goals` plug, whose container is never even resolved in that case --
/// spreading to nowhere is not a withdrawal, and there is no money to place.
///
/// A nonzero `Goals` plug with no unclaimed goal to land in is always an
/// error, but which message depends on the rest of the plan: reported on its
/// own when it is the only line with nowhere to go, folded into the broader
/// "nothing is configured" refusal when other lines are stuck alongside it --
/// a lone unconfigured line is ordinary, several at once with not one
/// transfer between them is a database nobody has pointed at anything. That
/// is the exact boundary the refusal below draws: it fires on zero transfers
/// *and* more than one stranded withdrawal, never on a single one.
pub fn plan(db: &Db, lines: &Lines) -> Result<Vec<Row>> {
    let mut transfers: Vec<Row> = Vec::new();
    let mut withdrawals: Vec<Row> = Vec::new();
    let mut plug_error: Option<anyhow::Error> = None;

    for line in Line::ALL {
        let cents = line.amount(lines);
        if cents == Cents::ZERO {
            continue;
        }
        match line.destination() {
            Destination::Spread => match spread_container(db)? {
                Some(to) => merge_transfer(&mut transfers, db, to, line, cents)?,
                None => {
                    plug_error = Some(anyhow!(
                        "the Goals plug is {} but no unallocated goal exists to \
                         spread it over",
                        crate::demo::figure(cents)
                    ));
                }
            },
            _ => match destination_account(db, line)? {
                Some(to) => merge_transfer(&mut transfers, db, to, line, cents)?,
                None => withdrawals.push(Row::Withdrawal { line, cents }),
            },
        }
    }

    if transfers.is_empty() && withdrawals.len() > 1 {
        bail!(
            "no Planning destination is configured, so every line would leave the tracked system"
        );
    }
    if let Some(err) = plug_error {
        return Err(err);
    }

    transfers.append(&mut withdrawals);
    ensure!(!transfers.is_empty(), "{NOTHING_TO_TRANSFER}");
    Ok(transfers)
}

/// Write a whole payday: every transfer's two legs and every withdrawal's
/// one, in a single transaction.
///
/// Atomic because a half-written payday is worse than none -- the balances it
/// leaves are wrong, and nothing on the ledger marks them as incomplete.
/// `txn::write_transfer` is what makes that possible: it does the sign-by-kind
/// work without opening a transaction of its own, since [`Db::transaction`]
/// is not reentrant.
///
/// Expects `rows` to be [`plan`]'s output. An empty slice commits an empty
/// transaction rather than erroring, and a hand-built zero-cent `Row` would
/// write two zero rows to the ledger -- neither shape `plan` itself
/// produces, since it drops every zero-valued line before a `Row` is built.
/// A `Row::Transfer` whose `to` equals `from` would likewise write a
/// self-cancelling pair of legs; `plan` cannot construct one today, because
/// every line's destination resolves to a goal's or account's own container,
/// never to `from` itself.
pub fn execute(db: &Db, from: AccountId, date: NaiveDate, rows: &[Row]) -> Result<()> {
    db.transaction(|db| {
        for row in rows {
            match row {
                Row::Transfer {
                    to, name, cents, ..
                } => txn::write_transfer(db, from, *to, date, *cents, name, name)?,
                // A withdrawal is one signed row, not half a transfer: the
                // money has left the tracked system, so there is no second
                // leg to write. Negated unconditionally, which is the cash
                // sign convention rather than the general per-kind one
                // `txn::write_transfer` applies -- correct because `from` is
                // always [`source`], the Everyday cash account, never a credit
                // account whose sign runs the other way.
                Row::Withdrawal { line, cents } => {
                    txn::insert(
                        db,
                        &NewTxn {
                            date,
                            cents: -*cents,
                            account_id: from,
                            description: line.label().to_string(),
                            recurring_txn_id: None,
                        },
                    )?;
                }
            }
        }
        Ok(())
    })
}

/// Which of `dates` the source ledger already carries this payday's rows on,
/// in the order asked.
///
/// A window rather than the single date the dialog opens on, because that
/// date is editable before the write: the case the warning exists for is a
/// first run dated wrongly and a second stepped onto the day it landed on,
/// and checking only the default checks the one date that case moves off.
/// Which days clashed is the caller's to report -- they are days the owner
/// cannot see on the form.
///
/// A warning rather than a block: these are ordinary ledger rows, deletable
/// one at a time, and a genuine second run -- the first one dated wrongly --
/// is a real case. Matched on description and amount, which is what the rows
/// carry; nothing marks a row as plan-generated.
///
/// Negates every row's amount unconditionally, which is the cash sign
/// convention rather than the general per-kind one `txn::write_transfer`
/// applies. `from` is a free parameter, but every real caller passes
/// [`source`], the Everyday cash account, so a leg leaving it is always negative.
pub fn already_written(
    db: &Db,
    from: AccountId,
    dates: &[NaiveDate],
    rows: &[Row],
) -> Result<Vec<NaiveDate>> {
    let mut clashing = Vec::new();
    for date in dates {
        let existing = txn::on_date(db, from, *date)?;
        let clashes = rows.iter().any(|row| {
            let (description, cents) = match row {
                Row::Transfer { name, cents, .. } => (name.clone(), -*cents),
                Row::Withdrawal { line, cents } => (line.label().to_string(), -*cents),
            };
            existing
                .iter()
                .any(|t| t.description == description && t.cents == cents)
        });
        if clashes {
            clashing.push(*date);
        }
    }
    Ok(clashing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{Group, Kind};
    use crate::db::goal::NewGoal;
    use crate::db::setting::key;
    use crate::db::txn;
    use crate::db::{self, GoalId, account, goal, setting};
    use crate::gate::Gate;
    use crate::rate::BasisPoints;
    use crate::test_support::day;

    /// A database shaped like the imported workbook: Everyday checking, Rainy Day and
    /// Brokerage containers, and one goal behind each configured line.
    fn configured() -> (db::Db, AccountId, AccountId) {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
        let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();

        let add = |container: AccountId, name: &str| -> GoalId {
            goal::insert(
                &db,
                &NewGoal {
                    name: name.to_string(),
                    container_account_id: container,
                    base_cents: Cents::from_dollars(1_000),
                    goal_date: None,
                    recurring_goal_id: None,
                    interest_eligible: true,
                    sort: 0,
                    taxed: false,
                },
            )
            .unwrap()
        };

        let bill_payments = add(savings, "Bill Payments");
        let housing = add(savings, "Housing");
        let roth = add(savings, "Roth IRA");
        add(savings, "Lego");
        add(savings, "Dropbox");
        let down_payment = add(brokerage, "Home Down Payment");
        let mom_and_dad = add(brokerage, "Mom & Dad");
        let emergency = add(brokerage, "Emergency Savings");

        let key = |line: Line| match line.destination() {
            Destination::Goal(key) => key,
            other => panic!("{line:?} resolves to {other:?}"),
        };
        setting::set(&db, key(Line::Bills), bill_payments).unwrap();
        setting::set(&db, key(Line::CurrentHousing), housing).unwrap();
        setting::set(&db, Gate::Roth.key(), roth).unwrap();
        setting::set(&db, key(Line::FutureHousing), down_payment).unwrap();
        setting::set(&db, key(Line::MomAndDad), mom_and_dad).unwrap();
        setting::set(&db, Gate::EmergencyFund.key(), emergency).unwrap();
        (db, savings, brokerage)
    }

    /// The `Key<GoalId>` behind a line, for the tests that set or clear one.
    fn key_of(line: Line) -> Key<GoalId> {
        match line.destination() {
            Destination::Goal(key) => key,
            other => panic!("{line:?} resolves to {other:?}"),
        }
    }

    fn insert_goal(db: &db::Db, container: AccountId, name: &str) -> GoalId {
        goal::insert(
            db,
            &NewGoal {
                name: name.to_string(),
                container_account_id: container,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap()
    }

    fn lines() -> Lines {
        Lines {
            bills: Cents::from_dollars(2_544),
            current_housing: Cents::from_dollars(693),
            goals: Cents::from_dollars(4_832),
            roth: Cents::ZERO,
            future_housing: Cents::from_dollars(4_830),
            mom_and_dad: Cents::from_dollars(461),
            emergency_fund: Cents::ZERO,
            retirement: Cents::from_dollars(2_070),
            investment: Cents::from_dollars(2_070),
        }
    }

    fn transfer_to(rows: &[Row], account: AccountId) -> Option<Cents> {
        rows.iter().find_map(|r| match r {
            Row::Transfer { to, cents, .. } if *to == account => Some(*cents),
            _ => None,
        })
    }

    fn withdrawals(rows: &[Row]) -> Vec<(Line, Cents)> {
        rows.iter()
            .filter_map(|r| match r {
                Row::Withdrawal { line, cents } => Some((*line, *cents)),
                _ => None,
            })
            .collect()
    }

    fn fund(db: &db::Db, id: GoalId, dollars: i64) {
        goal::insert_allocation(
            db,
            id,
            day(2026, 8, 1),
            Cents::from_dollars(dollars),
            Some("test"),
            None,
        )
        .unwrap();
    }

    fn goal_id(db: &db::Db, name: &str) -> GoalId {
        goal::all_with_balances(db)
            .unwrap()
            .into_iter()
            .find(|g| g.goal.name == name)
            .unwrap_or_else(|| panic!("no goal named {name:?}"))
            .goal
            .id
    }

    fn spread_names(db: &db::Db) -> Vec<String> {
        spread_goals(db, Reading::Strict)
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect()
    }

    /// The gap is measured against `lines.goals` itself and not against the
    /// transfer row carrying it, because a plug of nothing has no such row --
    /// `plan` skips a line at zero -- and that is the payday whose goals are
    /// worst served. Taking the row as the input would fade the warning out
    /// exactly as the condition it reports got worse.
    #[test]
    fn a_plug_short_of_the_asks_reports_the_gap_whatever_it_moves() {
        let asked = Cents::from_dollars(520);

        assert_eq!(
            unmet_asks(Cents::from_dollars(300), asked),
            Some(Cents::from_dollars(-220))
        );
        assert_eq!(
            unmet_asks(Cents::ZERO, asked),
            Some(Cents::from_dollars(-520))
        );
    }

    /// Covered is silence, and covered *exactly* is covered: a gap of zero
    /// is a figure that says nothing, drawn on every payday that balances.
    #[test]
    fn a_plug_that_meets_the_asks_reports_no_gap() {
        let moves = Cents::from_dollars(520);

        assert_eq!(unmet_asks(moves, moves), None);
        assert_eq!(unmet_asks(moves, Cents::from_dollars(300)), None);
    }

    /// The set and what each of its goals asks come off one read, so a
    /// screen quoting the total and a prefill dividing the plug cannot be
    /// answering two different questions about which goals.
    #[test]
    fn every_goal_the_plug_spreads_over_comes_back_with_what_it_asks() {
        let (db, savings, _) = configured();
        // Every goal `configured` leaves unclaimed is undated, so it has no
        // runway to divide and asks for nothing. One with a deadline a
        // single pay period out asks for the whole of what it lacks.
        goal::insert(
            &db,
            &NewGoal {
                name: "Bike".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(600),
                goal_date: Some(day(2026, 9, 5)),
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: false,
            },
        )
        .unwrap();

        let priced: Vec<(String, Cents)> = spread_asks(&db, day(2026, 8, 22), 14)
            .unwrap()
            .into_iter()
            .map(|(g, ask)| (g.name, ask))
            .collect();

        assert_eq!(
            priced,
            vec![
                ("Lego".to_string(), Cents::ZERO),
                ("Dropbox".to_string(), Cents::ZERO),
                ("Bike".to_string(), Cents::from_dollars(600)),
            ]
        );
    }

    /// The plug funds what still needs funding. A goal sitting at its target
    /// would otherwise take a share of every payday for ever.
    #[test]
    fn a_goal_that_has_met_its_target_is_not_spread_over() {
        let (db, _, _) = configured();
        fund(&db, goal_id(&db, "Lego"), 1_000);

        assert_eq!(spread_names(&db), vec!["Dropbox".to_string()]);
    }

    /// A taxed goal sitting at its base is not funded -- it is short by the
    /// tax. The plug's set is the goals that are still short, so it has to be
    /// in it, and it has to be offered a share. A second, already-met goal
    /// sits alongside it so `shares_of`'s "nothing short -> spread over
    /// everyone" fallback cannot paper over a reader that goes back to the
    /// base: under that filter neither goal reads as short, the fallback
    /// fires, and it returns both.
    #[test]
    fn a_taxed_goal_funded_to_its_base_is_still_in_the_plugs_set() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let taxed = goal::insert(
            &db,
            &NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        let met = goal::insert(
            &db,
            &NewGoal {
                name: "Lamp".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 1,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            taxed,
            day(2026, 1, 1),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
        goal::insert_allocation(
            &db,
            met,
            day(2026, 1, 1),
            Cents::from_dollars(500),
            None,
            None,
        )
        .unwrap();

        let spread = spread_goals(&db, Reading::Strict).unwrap();

        assert_eq!(
            spread.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![taxed],
            "a goal short by its tax must still be offered a share"
        );
    }

    /// The other half of the same rule: once the *taxed* figure is funded the
    /// goal needs nothing, so it drops out of the set -- and, because the same
    /// set decides where the plug lands, it stops pulling the spread into its
    /// container too.
    ///
    /// This case cannot discriminate a reader that still uses the base from
    /// one that uses the target: funded to the taxed figure, the goal reads
    /// as met either way, so a reader that regressed to the base would pass
    /// this test too. `a_taxed_goal_funded_to_its_base_is_still_in_the_plugs_set`
    /// above is the one that pins the target reading.
    #[test]
    fn a_taxed_goal_funded_to_its_taxed_figure_drops_out_of_the_plugs_set() {
        let db = db::open_in_memory().unwrap();
        setting::set(&db, key::TAX_RATE, BasisPoints(625)).unwrap();
        let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let taxed = goal::insert(
            &db,
            &NewGoal {
                name: "Couch".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();
        let short = goal::insert(
            &db,
            &NewGoal {
                name: "Rug".to_string(),
                container_account_id: savings,
                base_cents: Cents::from_dollars(500),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 1,
                taxed: false,
            },
        )
        .unwrap();
        goal::insert_allocation(&db, taxed, day(2026, 1, 1), Cents(106_500), None, None).unwrap();

        let spread = spread_goals(&db, Reading::Strict).unwrap();

        assert_eq!(
            spread.iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![short],
            "a goal at its taxed figure needs nothing"
        );
    }

    /// The bug this rule exists for: unfunding a line leaves its goal
    /// unclaimed, and a *met* goal in a second container was enough to make
    /// the plug ambiguous even though it could never receive a penny of it.
    #[test]
    fn a_met_goal_in_another_container_does_not_make_the_plug_ambiguous() {
        let (db, savings, _) = configured();
        let done = setting::get(&db, key_of(Line::FutureHousing))
            .unwrap()
            .unwrap();
        fund(&db, done, 1_000);
        setting::clear(&db, key_of(Line::FutureHousing)).unwrap();

        assert_eq!(spread_container(&db).unwrap(), Some(savings));
    }

    /// Everything being funded is a good problem and must not turn a payday
    /// into a refusal, so the set is every unclaimed goal rather than none --
    /// which is what still gives the plug a container to land in. Each of
    /// them asks for nothing, so the money ends up unallocated.
    #[test]
    fn a_plug_with_every_unclaimed_goal_met_still_lands_somewhere() {
        let (db, savings, _) = configured();
        for name in ["Lego", "Dropbox"] {
            fund(&db, goal_id(&db, name), 1_000);
        }

        assert_eq!(
            spread_names(&db),
            vec!["Lego".to_string(), "Dropbox".to_string()]
        );
        assert_eq!(spread_container(&db).unwrap(), Some(savings));
    }

    /// The two sets are not the same set. A met goal is still a perfectly
    /// good destination for a line -- which is exactly what makes "Home Down
    /// Payment?" worth offering on a Future Housing row that funds it no
    /// longer.
    #[test]
    fn a_met_goal_is_still_offered_as_a_suggestion() {
        let (db, _, _) = configured();
        let done = setting::get(&db, key_of(Line::FutureHousing))
            .unwrap()
            .unwrap();
        fund(&db, done, 1_000);
        setting::clear(&db, key_of(Line::FutureHousing)).unwrap();

        assert_eq!(
            suggest(&db, Line::FutureHousing).unwrap().map(|g| g.name),
            Some("Home Down Payment".to_string())
        );
    }

    /// The suggestion is what makes an unset line one keystroke to fix
    /// rather than a database nobody can read.
    #[test]
    fn an_unset_line_is_suggested_the_unclaimed_goal_its_name_matches() {
        let (db, _, _) = configured();
        setting::clear(&db, key_of(Line::Bills)).unwrap();

        let suggestion = suggest(&db, Line::Bills).unwrap();
        assert_eq!(
            suggestion.map(|g| g.name),
            Some("Bill Payments".to_string())
        );
    }

    /// "Lego" appears three times in the workbook. Offering the first would
    /// be picking between them by luck, and the whole reason name matching
    /// happens once, at import, is that nothing downstream may do that.
    #[test]
    fn a_suggestion_is_refused_when_two_unclaimed_goals_match() {
        let (db, savings, _) = configured();
        setting::clear(&db, key_of(Line::Bills)).unwrap();
        insert_goal(&db, savings, "Bill Payments (old)");

        assert!(suggest(&db, Line::Bills).unwrap().is_none());
    }

    /// A goal two lines both fund is funded twice, and the plug stops
    /// spreading over it as well -- so a claimed goal is never offered.
    #[test]
    fn a_goal_another_line_already_claims_is_never_suggested() {
        let (db, _, _) = configured();
        let bill_payments = setting::get(&db, key_of(Line::Bills)).unwrap().unwrap();
        setting::clear(&db, key_of(Line::Bills)).unwrap();
        setting::set(&db, key_of(Line::MomAndDad), bill_payments).unwrap();

        assert!(suggest(&db, Line::Bills).unwrap().is_none());
    }

    /// A suggestion answers "this line is unset"; a line already pointed
    /// somewhere is not asking.
    #[test]
    fn a_line_already_pointed_at_a_goal_is_offered_no_suggestion() {
        let (db, _, _) = configured();
        assert!(suggest(&db, Line::Bills).unwrap().is_none());
    }

    fn landing_of(db: &db::Db, line: Line) -> Landing {
        wiring(db)
            .unwrap()
            .into_iter()
            .find(|w| w.line == line)
            .expect("every line is wired")
            .landing
    }

    /// The container an account code names, as `wiring` reports it. Read
    /// back out of the database rather than written out, because a
    /// `Container` carries the account's id and its color as well as its
    /// name -- and the id is a rowid the fixture does not name.
    fn container(db: &db::Db, code: &str) -> Container {
        let account = account::by_code(db, code, Kind::Cash).unwrap().unwrap();
        Container {
            id: account.id,
            // A `String`, not a `label::Account`: the Planning screen
            // tints this through `planning::Tint` instead -- see
            // `src/tui/CLAUDE.md`'s account-color section.
            name: account.name.as_str().to_string(),
            color: account.color,
        }
    }

    #[test]
    fn wiring_reports_a_configured_line_with_its_goal_and_container() {
        let (db, _, _) = configured();
        assert_eq!(
            landing_of(&db, Line::MomAndDad),
            Landing::Goal {
                goal: "Mom & Dad".to_string(),
                container: container(&db, "BKR"),
            }
        );
    }

    /// Unset is a real, supported state for a destination: the money leaves
    /// the tracked system. The screen must not read it as an error.
    #[test]
    fn wiring_reports_an_unset_line_as_a_withdrawal() {
        let (db, _, _) = configured();
        assert_eq!(landing_of(&db, Line::Retirement), Landing::Withdrawal);
    }

    /// `plan` refuses outright on a dangling key, which is right -- it is
    /// about to move real money. The screen that has to *show* the corrupt
    /// row cannot refuse to draw itself, so `wiring` reports it instead,
    /// naming the key to fix.
    #[test]
    fn wiring_reports_a_dangling_key_rather_than_refusing() {
        let (db, _, _) = configured();
        setting::set(&db, key_of(Line::Bills), GoalId(9_999)).unwrap();

        assert_eq!(
            landing_of(&db, Line::Bills),
            Landing::Dangling {
                key: key_of(Line::Bills).name().to_string()
            }
        );
        assert!(plan(&db, &lines()).is_err(), "plan still refuses");
    }

    /// The same asymmetry over a goal that cannot derive a target. `plan` is
    /// about to spend the figure and refuses; `wiring` draws the plug against
    /// the base, because a Planning screen that refused here would be blank on
    /// exactly the database the owner opened it to understand.
    #[test]
    fn wiring_draws_the_plug_over_a_taxed_goal_with_no_rate_rather_than_refusing() {
        let (db, _, _) = configured();
        goal::insert(
            &db,
            &NewGoal {
                name: "Couch".to_string(),
                container_account_id: container(&db, "SAV").id,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 0,
                taxed: true,
            },
        )
        .unwrap();

        assert_eq!(
            landing_of(&db, Line::Goals),
            Landing::Spread {
                container: container(&db, "SAV")
            }
        );
        assert!(
            diagnose(&db, &lines()).is_ok(),
            "the detail panel draws too"
        );
        assert!(plan(&db, &lines()).is_err(), "plan still refuses");
    }

    #[test]
    fn wiring_reports_the_plug_as_the_container_it_spreads_over() {
        let (db, _, _) = configured();
        assert_eq!(
            landing_of(&db, Line::Goals),
            Landing::Spread {
                container: container(&db, "SAV")
            }
        );
    }

    /// The state the whole block exists to make visible: one unclaimed goal
    /// in the wrong container and the plug has nowhere single to go.
    #[test]
    fn wiring_reports_a_plug_spanning_two_containers_as_ambiguous() {
        let (db, _, _) = configured();
        setting::clear(&db, key_of(Line::MomAndDad)).unwrap();

        assert_eq!(
            landing_of(&db, Line::Goals),
            Landing::Ambiguous {
                containers: vec!["Rainy Day".to_string(), "Brokerage".to_string()],
            }
        );
    }

    /// A corrupt key is often the very thing that *causes* the ambiguity --
    /// the goal it should have claimed stays unclaimed in its own container
    /// -- so the two arrive together, and a `diagnose` that refuses on the
    /// first cannot explain the second. Refusing here blanks the whole
    /// Planning screen through `set_unavailable`, which is what `wiring`'s
    /// tolerant reading exists to prevent.
    #[test]
    fn a_dangling_key_alongside_an_ambiguous_plug_is_explained_not_refused() {
        let (db, _, brokerage) = configured();
        insert_goal(&db, brokerage, "Sabbatical");
        setting::set(&db, key_of(Line::Bills), GoalId(9_999)).unwrap();

        let text = diagnose(&db, &lines()).unwrap().join("\n");
        assert!(text.contains("bill_payments_id"), "{text}");
        assert!(text.contains("Brokerage"), "{text}");
    }

    /// The row is painted red from the landing alone, so anything that
    /// paints red must have something to say when `Enter` asks. A screen
    /// that alarms and then denies the alarm is worse than either.
    #[test]
    fn every_landing_that_paints_red_has_something_to_explain() {
        // A zero plug: `plan` skips zero lines entirely and resolves, but
        // `t` still refuses later, so the red row is telling the truth and
        // the panel has to back it up.
        let (db, _, brokerage) = configured();
        insert_goal(&db, brokerage, "Sabbatical");
        let zero = Lines {
            goals: Cents::ZERO,
            ..lines()
        };

        let ambiguous = wiring(&db)
            .unwrap()
            .into_iter()
            .find(|w| w.line == Line::Goals)
            .unwrap();
        assert!(ambiguous.landing.breaks_the_plan(), "the row is red");
        assert!(plan(&db, &zero).is_ok(), "and yet the plan resolves");
        assert!(
            !diagnose(&db, &zero).unwrap().is_empty(),
            "so Enter must not answer \"nothing to explain\""
        );
    }

    /// The row says what is wrong in a cell fifty columns wide; this says
    /// which goal to go and fix.
    #[test]
    fn diagnose_names_the_containers_and_the_goals_in_the_smaller_one() {
        let (db, _, _) = configured();
        setting::clear(&db, key_of(Line::MomAndDad)).unwrap();

        let text = diagnose(&db, &lines()).unwrap().join("\n");
        assert!(text.contains("Rainy Day"), "{text}");
        assert!(text.contains("Brokerage"), "{text}");
        assert!(text.contains("Mom & Dad"), "{text}");
    }

    /// The panel this text fills is drawn by the Planning screen, so the
    /// figure in it is on screen exactly as a column is -- and this module
    /// sits below `tui`, which is why `demo` sits at the crate root.
    #[test]
    fn a_demo_blocks_the_plug_the_diagnosis_quotes() {
        crate::demo::install(true);
        let (db, _, _) = configured();
        setting::clear(&db, key_of(Line::MomAndDad)).unwrap();

        let text = diagnose(&db, &lines()).unwrap().join("\n");
        assert!(!text.contains("4,832"), "the plug survived: {text}");
        assert!(text.contains("██████"), "nothing was blocked: {text}");
        assert!(
            text.contains("Rainy Day"),
            "the containers must stay: {text}"
        );
    }

    /// Nothing is wrong, so there is nothing to explain -- and an empty
    /// panel is what stops `Enter` offering one.
    #[test]
    fn diagnose_of_a_resolved_plan_is_empty() {
        let (db, _, _) = configured();
        assert!(diagnose(&db, &lines()).unwrap().is_empty());
    }

    /// Four lines land in Rainy Day and three in Brokerage, and each account gets
    /// one transfer carrying their sum -- not one transfer per line.
    #[test]
    fn lines_sharing_a_destination_make_one_transfer() {
        let (db, savings, brokerage) = configured();
        let rows = plan(&db, &lines()).unwrap();

        assert_eq!(
            transfer_to(&rows, savings),
            Some(Cents::from_dollars(8_069))
        );
        assert_eq!(
            transfer_to(&rows, brokerage),
            Some(Cents::from_dollars(5_291))
        );
        assert_eq!(
            rows.iter()
                .filter(|r| matches!(r, Row::Transfer { .. }))
                .count(),
            2
        );
    }

    /// Retirement and Investment go to different places in the real world, so
    /// they are two rows on the ledger rather than one lump of 4,140.
    #[test]
    fn each_unconfigured_line_is_its_own_withdrawal() {
        let (db, _, _) = configured();
        let rows = plan(&db, &lines()).unwrap();
        assert_eq!(
            withdrawals(&rows),
            vec![
                (Line::Retirement, Cents::from_dollars(2_070)),
                (Line::Investment, Cents::from_dollars(2_070)),
            ]
        );
    }

    /// The switch the owner will actually make: when the down payment is
    /// funded, nulling Future Housing's key turns 4,830 of Brokerage's
    /// transfer into a withdrawal. No code change, and Brokerage drops to
    /// Mom & Dad alone.
    #[test]
    fn clearing_future_housings_key_moves_its_money_out_of_brokerage() {
        let (db, _, brokerage) = configured();
        let Destination::Goal(key) = Line::FutureHousing.destination() else {
            panic!("Future Housing is goal-backed");
        };
        // The down payment goal is funded and closed, same as the owner
        // would do -- clearing the key alone would leave it open and
        // unclaimed, spanning Brokerage and Rainy Day's Lego/Dropbox together
        // and tripping the container-span error a genuinely stray goal
        // should trip.
        let down_payment = setting::get(&db, key).unwrap().unwrap();
        goal::close(&db, down_payment).unwrap();
        setting::clear(&db, key).unwrap();

        let rows = plan(&db, &lines()).unwrap();

        assert_eq!(
            transfer_to(&rows, brokerage),
            Some(Cents::from_dollars(461))
        );
        assert!(
            withdrawals(&rows).contains(&(Line::FutureHousing, Cents::from_dollars(4_830))),
            "{rows:?}"
        );
    }

    /// A key pointing at a goal that is gone is a corrupt database, not an
    /// unconfigured line. Degrading to a withdrawal would move real money out
    /// of the tracked system on the strength of a dangling row.
    #[test]
    fn a_key_pointing_at_a_deleted_goal_is_an_error_naming_the_key() {
        let (db, _, _) = configured();
        let Destination::Goal(key) = Line::MomAndDad.destination() else {
            panic!("Mom & Dad is goal-backed");
        };
        setting::set(&db, key, GoalId(9_999)).unwrap();

        let err = plan(&db, &lines()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("planning.goal.mom_and_dad_id"), "{text}");
    }

    /// The same for an account key: a dangling account id must not read as
    /// "not configured".
    #[test]
    fn a_key_pointing_at_a_deleted_account_is_an_error_naming_the_key() {
        let (db, _, _) = configured();
        let Destination::Account(key) = Line::Retirement.destination() else {
            panic!("Retirement is account-backed");
        };
        setting::set(&db, key, AccountId(9_999)).unwrap();

        let err = plan(&db, &lines()).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("planning.account.retirement_id"), "{text}");
    }

    /// The plug goes to the container holding the goals no line claims. Bill
    /// Payments and Housing are claimed, so they are not what decides it and
    /// they are not funded twice.
    #[test]
    fn the_plug_lands_in_the_container_of_the_goals_no_line_claims() {
        let (db, savings, _) = configured();
        assert_eq!(spread_container(&db).unwrap(), Some(savings));

        let names: Vec<String> = unclaimed_goals(&db)
            .unwrap()
            .into_iter()
            .map(|g| g.name)
            .collect();
        assert_eq!(names, vec!["Lego".to_string(), "Dropbox".to_string()]);
    }

    /// The plug is a single amount and there is no rule for dividing it, so
    /// unclaimed goals in two containers is an error rather than a guess.
    /// The message names them: the owner reading it has to find the goal in
    /// the wrong container before they can act on it.
    #[test]
    fn unclaimed_goals_spanning_two_containers_is_an_error() {
        let (db, _, brokerage) = configured();
        goal::insert(
            &db,
            &NewGoal {
                name: "Sabbatical".to_string(),
                container_account_id: brokerage,
                base_cents: Cents::from_dollars(1_000),
                goal_date: None,
                recurring_goal_id: None,
                interest_eligible: true,
                sort: 9,
                taxed: false,
            },
        )
        .unwrap();

        let err = format!("{:#}", plan(&db, &lines()).unwrap_err());
        assert!(
            err.contains("Rainy Day") && err.contains("Brokerage"),
            "{err}"
        );
    }

    /// Spreading to nowhere is not a withdrawal: with no unclaimed goal, a
    /// non-zero plug has no destination and must say so.
    #[test]
    fn a_non_zero_plug_with_no_unclaimed_goal_is_an_error() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let l = Lines {
            goals: Cents::from_dollars(100),
            ..Lines::default()
        };

        let err = plan(&db, &l).unwrap_err();
        assert!(err.to_string().contains("no unallocated goal"), "{err}");
    }

    /// A plan with nothing in any line at all -- not merely nowhere to go --
    /// refuses outright: every group is zero, so there is nothing to write
    /// and no rows to hide.
    #[test]
    fn a_plan_with_every_line_zero_is_refused_as_nothing_to_transfer() {
        let (db, _, _) = configured();
        let rows = plan(&db, &Lines::default()).unwrap_err();
        // Every line zero means every group zero, which is the unconfigured
        // case below -- asserted there. Here, only that it does not silently
        // produce rows.
        assert_eq!(rows.to_string(), NOTHING_TO_TRANSFER, "{rows}");
    }

    /// A zero line inside an otherwise non-zero group is dropped before the
    /// transfer is built at all -- `lines_sharing_a_destination_make_one_
    /// transfer` only checks the summed cents, which a zero line would not
    /// change.
    #[test]
    fn a_zero_line_among_non_zero_ones_is_absent_from_its_transfers_lines() {
        let (db, savings, _) = configured();
        let rows = plan(&db, &lines()).unwrap();

        let savings_lines = rows
            .iter()
            .find_map(|r| match r {
                Row::Transfer { to, lines, .. } if *to == savings => Some(lines.clone()),
                _ => None,
            })
            .expect("no Rainy Day transfer");
        assert!(
            !savings_lines.iter().any(|(line, _)| *line == Line::Roth),
            "{savings_lines:?}"
        );
    }

    /// Every line resolving to a withdrawal means nothing is configured at
    /// all, and writing every one of them out of that database as an outgoing
    /// row would be a disaster dressed as a payday.
    #[test]
    fn a_database_with_no_configured_destination_is_refused() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();

        let err = plan(&db, &lines()).unwrap_err();
        assert!(err.to_string().contains("no Planning destination"), "{err}");
    }

    /// One line with nowhere to go is not an unconfigured database -- it is
    /// one correctly-labelled row behind a confirm modal, the ordinary case
    /// of an owner who has not yet set up a Retirement account.
    #[test]
    fn a_lone_unconfigured_line_is_a_withdrawal_rather_than_a_refusal() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let l = Lines {
            retirement: Cents::from_dollars(100),
            ..Lines::default()
        };

        let rows = plan(&db, &l).unwrap();
        assert_eq!(
            withdrawals(&rows),
            vec![(Line::Retirement, Cents::from_dollars(100))]
        );
    }

    /// Two lines with nowhere to go and no transfer to show for either is
    /// the shape the refusal exists for, even short of every line failing.
    #[test]
    fn two_stranded_lines_with_no_transfer_between_them_are_refused() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        let l = Lines {
            retirement: Cents::from_dollars(100),
            investment: Cents::from_dollars(100),
            ..Lines::default()
        };

        let err = plan(&db, &l).unwrap_err();
        assert!(err.to_string().contains("no Planning destination"), "{err}");
    }

    /// `source` is the account every transfer leaves from, resolved the same
    /// way every other account lookup in this module is: by code and kind,
    /// never by a name a screen might have relabelled.
    #[test]
    fn source_is_the_account_in_the_checking_band() {
        let db = db::open_in_memory().unwrap();
        let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
        account::set_group(&db, checking, Group::Checking).unwrap();
        assert_eq!(source(&db).unwrap(), checking);
    }

    /// A database with nothing in the Checking band cannot fund a payday at
    /// all, and that has to say so rather than resolve to some other account.
    /// A fresh import is exactly that database: every cash account starts in
    /// its kind's default band, which is Savings.
    #[test]
    fn source_with_no_checking_account_is_an_error() {
        let db = db::open_in_memory().unwrap();
        account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 0).unwrap();
        let err = source(&db).unwrap_err();
        assert!(err.to_string().contains("Checking band"), "{err}");
    }

    /// Two accounts in the band is an ambiguity only the owner can settle:
    /// a transfer leaves from one account, and picking either would move real
    /// money out of an account the plan did not count.
    #[test]
    fn source_with_two_checking_accounts_is_an_error() {
        let db = db::open_in_memory().unwrap();
        for (code, name, sort) in [("CHK", "Everyday", 0), ("SAV", "Rainy Day", 1)] {
            let id = account::insert(&db, code, name, Kind::Cash, sort).unwrap();
            account::set_group(&db, id, Group::Checking).unwrap();
        }
        let err = source(&db).unwrap_err();
        assert!(err.to_string().contains("Everyday"), "{err}");
        assert!(err.to_string().contains("Rainy Day"), "{err}");
    }

    /// `Row::cents` reads the same field regardless of which variant it is,
    /// so a caller totalling a plan does not need to match on `Row` itself.
    #[test]
    fn cents_reads_either_row_variant() {
        let transfer = Row::Transfer {
            to: AccountId(1),
            name: "Rainy Day".to_string(),
            color: None,
            cents: Cents::from_dollars(100),
            lines: vec![(Line::Bills, Cents::from_dollars(100))],
        };
        let withdrawal = Row::Withdrawal {
            line: Line::Retirement,
            cents: Cents::from_dollars(50),
        };
        assert_eq!(transfer.cents(), Cents::from_dollars(100));
        assert_eq!(withdrawal.cents(), Cents::from_dollars(50));
    }

    /// A half-written payday is worse than none: the balances it leaves are
    /// wrong and nothing marks them as incomplete. One bad row must take the
    /// whole batch with it.
    #[test]
    fn one_failing_row_rolls_back_the_whole_payday() {
        let (db, savings, _) = configured();
        let checking = source(&db).unwrap();
        let rows = vec![
            Row::Transfer {
                to: savings,
                name: "Rainy Day".to_string(),
                color: None,
                cents: Cents::from_dollars(8_069),
                lines: vec![(Line::Bills, Cents::from_dollars(8_069))],
            },
            Row::Transfer {
                to: AccountId(9_999),
                name: "Nowhere".to_string(),
                color: None,
                cents: Cents::from_dollars(100),
                lines: vec![(Line::Roth, Cents::from_dollars(100))],
            },
        ];

        assert!(execute(&db, checking, day(2026, 8, 20), &rows).is_err());
        assert_eq!(
            txn::count(&db).unwrap(),
            0,
            "a leg survived a failed payday"
        );
    }

    /// Both legs of every transfer, plus one row per withdrawal. The
    /// withdrawal is signed out of checking and has no second leg -- the
    /// money has left the tracked system.
    #[test]
    fn a_payday_writes_both_legs_of_each_transfer_and_one_row_per_withdrawal() {
        let (db, _, _) = configured();
        let checking = source(&db).unwrap();
        let rows = plan(&db, &lines()).unwrap();

        execute(&db, checking, day(2026, 8, 20), &rows).unwrap();

        // Two transfers (two legs each) and two withdrawals.
        assert_eq!(txn::count(&db).unwrap(), 6);
        assert_eq!(
            txn::balance_at(&db, checking, day(2026, 8, 20)).unwrap(),
            -Cents::from_dollars(8_069 + 5_291 + 2_070 + 2_070)
        );
    }

    /// A withdrawal's description is the line's own label, so the Everyday ledger
    /// reads `Retirement` rather than a second row indistinguishable from
    /// the first.
    #[test]
    fn a_withdrawal_is_described_by_its_line() {
        let (db, _, _) = configured();
        let checking = source(&db).unwrap();
        let rows = plan(&db, &lines()).unwrap();

        execute(&db, checking, day(2026, 8, 20), &rows).unwrap();

        let filter = txn::Filter {
            kind: Kind::Cash,
            account_id: None,
            from: day(2026, 1, 1),
            to: day(2026, 12, 31),
        };
        let descriptions: Vec<String> = txn::list(&db, &filter)
            .unwrap()
            .into_iter()
            .map(|t| t.description)
            .collect();
        assert!(
            descriptions.contains(&"Retirement".to_string()),
            "{descriptions:?}"
        );
        assert!(
            descriptions.contains(&"Investment".to_string()),
            "{descriptions:?}"
        );
    }

    /// Re-firing is a real case -- a corrected date -- so it warns rather
    /// than blocks, and the warning has to be able to tell.
    #[test]
    fn a_payday_already_on_the_ledger_is_detected() {
        let (db, _, _) = configured();
        let checking = source(&db).unwrap();
        let rows = plan(&db, &lines()).unwrap();
        let date = day(2026, 8, 20);

        assert!(
            already_written(&db, checking, &[date], &rows)
                .unwrap()
                .is_empty()
        );
        execute(&db, checking, date, &rows).unwrap();
        assert_eq!(
            already_written(&db, checking, &[date], &rows).unwrap(),
            vec![date]
        );
        assert!(
            already_written(&db, checking, &[day(2026, 8, 21)], &rows)
                .unwrap()
                .is_empty()
        );
    }

    /// The dates come back in the order they were asked about, and only the
    /// ones that clash: the warning names days the owner cannot see, so it
    /// has to say which.
    #[test]
    fn only_the_dates_that_clash_come_back_and_in_the_order_asked() {
        let (db, _, _) = configured();
        let checking = source(&db).unwrap();
        let rows = plan(&db, &lines()).unwrap();
        let scanned = [
            day(2026, 8, 19),
            day(2026, 8, 20),
            day(2026, 8, 21),
            day(2026, 8, 24),
        ];
        execute(&db, checking, day(2026, 8, 24), &rows).unwrap();
        execute(&db, checking, day(2026, 8, 20), &rows).unwrap();

        assert_eq!(
            already_written(&db, checking, &scanned, &rows).unwrap(),
            vec![day(2026, 8, 20), day(2026, 8, 24)]
        );
    }
}
