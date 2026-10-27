//! The Savings tab: every container's goals, and what it has left over.

use crate::money::Cents;
use crate::palette;
use crate::palette::Sense;
use crate::rate::Percent;
use crate::report::view::{
    Accounts, Block, Cell, History, NO_ALLOCATIONS, Row as ViewRow, TabView, money, whole,
};
use crate::report::{Container, Goal};
use crate::savings::unallocated;

/// The Savings table's own header, in `src/tui/savings.rs`'s column order and
/// wording minus two columns: the leading `Account`, because the report
/// groups a container's rows under its own heading instead of repeating the
/// container on every row; and the trailing `$/Pay`, because six columns do
/// not fit a phone and what a goal asks per paycheck is a figure for the
/// payday, which is spent at the screen rather than read off the page.
/// `src/report/` cannot name `tui` to read the labels off it directly, so the
/// two sinks are kept in agreement by eye instead.
const HEADER: [&str; 5] = ["Goal", "Current", "Goal", "%", "Goal Date"];

/// A goal's percentage cell, on [`palette::percent`]'s ramp: red at nothing,
/// yellow at halfway, green at funded. The Savings screen's `%` column reads
/// the same three stops, so a goal is the same shade in either sink. The one
/// tone on the page that is a `#rrggbb` rather than a class, the ramp being
/// continuous.
///
/// A goal with no positive target has no percentage to place on the ramp, so
/// its `--` stays plain rather than being colored as if it were at zero -- the
/// same split the screen makes over its em dash.
fn percent_cell(percent: Option<Percent>) -> Cell {
    match percent {
        Some(p) => Cell::Toned(format!("{}%", p.0), palette::hex(palette::percent(p))),
        None => Cell::Text("--".to_string()),
    }
}

/// What `Enter` opens on a Savings row: `Earlier` ahead of the listed
/// allocations, then the total the balance beside the name is.
///
/// The amounts keep their cents where the table drops them: the rows worth
/// auditing are the interest postings and the import's, which carry cents,
/// and the total has to land on the balance rather than near it. That is
/// also why the allocations older than the ledgers reach are one `Earlier`
/// line ahead of the rest rather than left off. Nothing allocated at all is
/// `rows` empty and `empty` set, so the script reads the sentence from the
/// payload.
fn view_history(goal: &Goal) -> History {
    let row = &goal.row;
    let note = row.note.clone();
    if goal.history.is_empty() && goal.earlier.is_none() {
        return History {
            name: row.name.clone(),
            note,
            rows: Vec::new(),
            empty: Some(NO_ALLOCATIONS),
        };
    }
    let line = |cells: [String; 2], cents: Cents, k: Option<&str>| {
        let [a, b] = cells;
        ViewRow {
            c: vec![
                Cell::Text(a),
                Cell::Text(b),
                money(Sense::Natural, cents.to_string(), cents),
            ],
            k: k.map(str::to_string),
            h: None,
            d: None,
            m: None,
        }
    };
    let mut rows = Vec::new();
    if let Some(e) = &goal.earlier {
        rows.push(line(
            [
                "Earlier".to_string(),
                format!(
                    "{} allocations before {}",
                    e.count,
                    e.before.format("%b %Y")
                ),
            ],
            e.cents,
            None,
        ));
    }
    for a in &goal.history {
        rows.push(line(
            [
                a.date.to_string(),
                crate::description::render(a.note.as_deref().unwrap_or_default()).into_owned(),
            ],
            a.cents,
            None,
        ));
    }
    let total = goal.history.iter().fold(
        goal.earlier.as_ref().map_or(Cents::ZERO, |e| e.cents),
        |sum, a| sum + a.cents,
    );
    rows.push(line(
        ["Total".to_string(), String::new()],
        total,
        Some("tot"),
    ));
    History {
        name: row.name.clone(),
        note,
        rows,
        empty: None,
    }
}

/// The Savings tab: per container a heading and its goals, each goal
/// pointing at its history in `histories`, and the Unallocated remainder
/// below -- through [`unallocated`], the rule the screen's own footer reads,
/// so the two sinks cannot quote different remainders.
pub(super) fn view(
    containers: &[Container],
    acc: &mut Accounts,
    histories: &mut Vec<History>,
) -> TabView {
    let mut blocks = Vec::new();
    for container in containers {
        blocks.push(Block::Heading {
            text: acc.cell(&container.account),
        });
        let mut rows = Vec::new();
        for goal in &container.rows {
            let r = &goal.row;
            let k = [(r.favorite, "fav"), (r.expired, "expired")]
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, name)| *name)
                .collect::<Vec<_>>()
                .join(" ");
            histories.push(view_history(goal));
            rows.push(ViewRow {
                c: vec![
                    Cell::Text(r.name.clone()),
                    whole(r.current),
                    whole(r.goal),
                    percent_cell(r.percent),
                    Cell::Text(r.goal_date.map(|d| d.to_string()).unwrap_or_default()),
                ],
                k: (!k.is_empty()).then_some(k),
                h: Some(histories.len() - 1),
                d: None,
                m: r.percent.map(|p| p.0.clamp(0, 100) as u8),
            });
        }
        let blank = || Cell::Text(String::new());
        rows.push(ViewRow {
            c: vec![
                Cell::Text("Unallocated".to_string()),
                whole(unallocated(container.excess)),
                blank(),
                blank(),
                blank(),
            ],
            k: Some("unalloc".to_string()),
            h: None,
            d: None,
            m: None,
        });
        blocks.push(Block::Table {
            class: "sv",
            head: HEADER.map(str::to_string).to_vec(),
            rows,
        });
    }
    TabView {
        id: "savings",
        blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, row, snapshot};
    use super::view;
    use crate::money::Cents;
    use crate::palette;
    use crate::rate::Percent;
    use crate::report::view::Accounts;
    use serde_json::json;

    /// The view of a snapshot's containers, with its histories alongside.
    fn built(s: &crate::report::Snapshot) -> (serde_json::Value, serde_json::Value) {
        let mut histories = Vec::new();
        let tab = view(&s.containers, &mut Accounts::new(), &mut histories);
        (json(&tab), json(&histories))
    }

    #[test]
    fn a_favorite_goal_is_marked_and_an_expired_one_flagged() {
        let mut fav = row("Rainy Day", 500, 1_000);
        fav.favorite = true;
        let mut late = row("Nest Egg", 500, 1_000);
        late.expired = true;
        let mut both = row("Holdings", 500, 1_000);
        both.favorite = true;
        both.expired = true;
        let plain = row("Everyday", 500, 1_000);
        let (tab, _) = built(&snapshot(vec![fav, late, both, plain], 0));
        let rows = &tab["blocks"][1]["rows"];
        assert_eq!(rows[0]["k"], "fav");
        assert_eq!(rows[1]["k"], "expired");
        assert_eq!(rows[2]["k"], "fav expired");
        assert!(rows[3].get("k").is_none());
        assert_eq!(rows[0]["h"], 0);
        assert_eq!(rows[3]["h"], 3);
    }

    #[test]
    fn a_goals_history_foots_to_its_balance_with_earlier_folded_in() {
        let mut s = snapshot(vec![row("Rainy Day", 650, 1_000)], 0);
        s.containers[0].rows[0].earlier = Some(crate::report::Earlier {
            before: crate::test_support::day(2026, 5, 1),
            count: 4,
            cents: Cents::from_dollars(150),
        });
        let (_, histories) = built(&s);
        let rows = histories[0]["rows"].as_array().unwrap();
        assert_eq!(rows[0]["c"][0], "Earlier");
        assert_eq!(rows[0]["c"][1], "4 allocations before May 2026");
        // 300 + 200 listed, 150 folded in: the cells are what `Cents` prints.
        let shown = |dollars: i64| json!(Cents::from_dollars(dollars).to_string());
        assert_eq!(rows[0]["c"][2], shown(150));
        assert_eq!(rows[1]["c"][2], shown(300));
        assert_eq!(rows[2]["c"][2], shown(200));
        assert_eq!(
            s.containers[0].rows[0].row.current,
            Cents::from_dollars(650)
        );
        let total = rows.last().unwrap();
        assert_eq!(total["k"], "tot");
        assert_eq!(total["c"][0], "Total");
        assert_eq!(total["c"][2], shown(650));
        assert_eq!(rows[1]["c"][1], "Paycheck");
        assert_eq!(rows[2]["c"][1], "\u{2014}");
    }

    #[test]
    fn a_goals_note_is_carried_exactly_as_stored() {
        // A whitespace-only note is what `description::render` would turn into
        // an em dash, so only an untouched note passes.
        for note in ["Roof <first> & \u{2014}", "  "] {
            let mut s = snapshot(vec![row("Rainy Day", 500, 1_000)], 0);
            s.containers[0].rows[0].row.note = Some(note.to_string());
            let (_, histories) = built(&s);
            assert_eq!(histories[0]["note"], note);
        }
    }

    #[test]
    fn a_goal_with_no_allocations_says_so() {
        let mut s = snapshot(vec![row("Rainy Day", 0, 1_000)], 0);
        s.containers[0].rows[0].history.clear();
        s.containers[0].rows[0].row.note = Some("Roof".into());
        let (_, histories) = built(&s);
        assert_eq!(histories[0]["rows"], json!([]));
        assert_eq!(histories[0]["empty"], "No allocations yet.");
        assert_eq!(histories[0]["note"], "Roof");
    }

    #[test]
    fn the_funding_percent_is_colored_by_the_ramp() {
        let mut r = row("Rainy Day", 400, 1_000);
        r.percent = Some(Percent(40));
        let (tab, _) = built(&snapshot(vec![r], 0));
        let goal = &tab["blocks"][1]["rows"][0];
        assert_eq!(
            goal["c"][3],
            json!(["40%", palette::hex(palette::percent(Percent(40)))])
        );
        assert_eq!(goal["m"], 40);
    }

    #[test]
    fn a_goal_with_no_percentage_has_a_plain_dash_and_no_meter() {
        let mut r = row("Rainy Day", 400, 1_000);
        r.percent = None;
        let (tab, _) = built(&snapshot(vec![r], 0));
        let goal = &tab["blocks"][1]["rows"][0];
        assert_eq!(goal["c"][3], "--");
        assert!(goal.get("m").is_none());
    }

    #[test]
    fn the_meter_is_clamped_to_a_hundred() {
        let mut r = row("Rainy Day", 2_000, 1_000);
        r.percent = Some(Percent(200));
        let (tab, _) = built(&snapshot(vec![r], 0));
        assert_eq!(tab["blocks"][1]["rows"][0]["m"], 100);
    }

    #[test]
    fn unallocated_closes_each_container() {
        let (tab, _) = built(&snapshot(vec![row("Rainy Day", 500, 1_000)], 0));
        assert_eq!(tab["blocks"][0]["t"], "Heading");
        assert!(tab["blocks"][0]["text"].is_number());
        let t = &tab["blocks"][1];
        assert_eq!(t["class"], "sv");
        assert_eq!(
            t["head"],
            json!(["Goal", "Current", "Goal", "%", "Goal Date"])
        );
        let last = t["rows"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["k"], "unalloc");
        assert_eq!(last["c"], json!(["Unallocated", "0", "", "", ""]));
    }

    /// Allocations summed into `Earlier` are still allocations: the history
    /// lists that line and its total rather than saying there are none.
    #[test]
    fn a_goal_with_only_earlier_allocations_does_not_say_it_has_none() {
        let mut s = snapshot(vec![row("Rainy Day", 150, 1_000)], 0);
        let goal = &mut s.containers[0].rows[0];
        goal.history.clear();
        goal.earlier = Some(crate::report::Earlier {
            before: crate::test_support::day(2026, 5, 1),
            count: 4,
            cents: Cents::from_dollars(150),
        });
        let (_, histories) = built(&s);
        assert!(histories[0].get("empty").is_none(), "{}", histories[0]);
        let rows = histories[0]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["c"], json!(["Total", "", "150.00"]));
    }

    #[test]
    fn a_container_with_no_open_goals_still_reports_its_unallocated_remainder() {
        let (tab, histories) = built(&snapshot(vec![], 0));
        assert!(
            tab["blocks"][0]["text"].is_number(),
            "the container vanished"
        );
        let rows = tab["blocks"][1]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["c"][0], "Unallocated");
        assert_eq!(histories, json!([]));
    }

    /// A database with no containers still has a Savings tab, over nothing.
    #[test]
    fn a_snapshot_with_no_containers_still_renders() {
        let mut s = snapshot(vec![], 0);
        s.containers.clear();
        let (tab, _) = built(&s);
        assert_eq!(tab["id"], "savings");
        assert_eq!(tab["blocks"], json!([]));
    }
}
