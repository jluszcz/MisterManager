//! The Savings tab: every container's goals, and what it has left over.

use super::{account, escape, positional_money, whole_money};
use crate::money::Cents;
use crate::palette;
use crate::palette::Sense;
use crate::rate::Percent;
use crate::report::view::{
    Accounts, Block, Cell, History, NO_ALLOCATIONS, Row as ViewRow, TabView, money, whole,
};
use crate::report::{Container, Goal};
use crate::savings::{Row, unallocated};

/// The row's `<tr>` classes: `fav` for the owner's highlight, `expired` for a
/// goal past its date and still short. Either, both, or neither.
fn row_classes(row: &Row) -> String {
    let mut classes = Vec::new();
    if row.favorite {
        classes.push("fav");
    }
    if row.expired {
        classes.push("expired");
    }
    if classes.is_empty() {
        String::new()
    } else {
        format!(" class=\"{}\"", classes.join(" "))
    }
}

/// How funded a goal is, on [`palette::percent`]'s ramp: red at nothing,
/// yellow at halfway, green at funded. The Savings screen's `%` column reads
/// the same three stops, so a goal is the same shade in either sink.
///
/// A goal with no positive target has no percentage to place on the ramp, so
/// its `--` stays plain rather than being colored as if it were at zero -- the
/// same split the screen makes over its em dash.
fn percent(percent: Option<Percent>) -> String {
    match percent {
        Some(p) => format!(
            "<td class=\"n\" style=\"color:{}\">{}%</td>",
            palette::hex(palette::percent(p)),
            p.0
        ),
        None => "<td class=\"n\">--</td>".to_string(),
    }
}

/// The Savings table's own header, in `src/tui/savings.rs`'s column order and
/// wording minus two columns: the leading `Account`, because the report
/// groups a container's rows under its own heading instead of repeating the
/// container on every row; and the trailing `$/Pay`, because six columns do
/// not fit a phone and what a goal asks per paycheck is a figure for the
/// payday, which is spent at the screen rather than read off the page.
/// `src/report/` cannot name `tui` to read the labels off it directly, so the
/// two sinks are kept in agreement by eye instead.
const HEADER: [&str; 5] = ["Goal", "Current", "Goal", "%", "Goal Date"];

/// One container: its goals, and the Unallocated remainder below them.
///
/// The widest table on the page -- five columns, and a phone -- so the goal
/// name is the column that gives way (`w`) while the date and the
/// figures hold their line. `Unallocated` is ours rather than the owner's and
/// takes no `w`: it is a word the column is sized to fit, not one it may break
/// to make room for something else.
fn section(container: &Container) -> String {
    let header: String = HEADER
        .iter()
        .enumerate()
        .map(|(i, h)| {
            // The name and the goal date read as prose; the rest are money or
            // a percentage, right-aligned the same way their column's cells
            // are.
            let class = if i == 0 || i == 4 { "" } else { " class=\"n\"" };
            format!("<th{class}>{h}</th>")
        })
        .collect();
    let mut rows = format!("<thead><tr>{header}</tr></thead>");
    for Goal { row: r, .. } in &container.rows {
        let goal_date = r
            .goal_date
            .map(|d| escape(&d.to_string()))
            .unwrap_or_default();
        rows.push_str(&format!(
            "<tr{}><td class=\"w\"><button type=\"button\" class=\"goal\" \
             popovertarget=\"{}\">{}</button></td>{}{}{}<td class=\"d\">{goal_date}</td></tr>",
            row_classes(r),
            history_id(r),
            escape(&r.name),
            whole_money(r.current),
            whole_money(r.goal),
            percent(r.percent),
        ));
    }
    // Named rather than left to `whole_money`, which would do the same
    // arithmetic: this is the rule the screen's own footer reads, and one
    // function is what stops the two sinks quoting different remainders.
    let excess = unallocated(container.excess);
    // Five cells, matching every goal row above: the excess is a money figure
    // and Current is where a reader looks for "how much is sitting here".
    rows.push_str(&format!(
        "<tr><td>Unallocated</td>{}<td class=\"n\"></td><td class=\"n\"></td><td class=\"d\"></td></tr>",
        whole_money(excess)
    ));
    let histories: String = container.rows.iter().map(history).collect();
    format!(
        "<h3>{}</h3><table>{rows}</table>{histories}",
        account(&container.account)
    )
}

/// The id a goal's history is opened by. A goal id rather than its name,
/// which is neither unique nor safe to put in an attribute.
fn history_id(row: &Row) -> String {
    format!("history-{}", row.goal_id.0)
}

/// What `Enter` opens on a Savings row: the goal's note, then every
/// allocation its balance is the sum of, footed by the total -- which is the
/// `Current` beside the name, to the cent.
///
/// A `popover` rather than a radio and a `:checked ~` rule, because the
/// table it is opened from sits between any radio and the element it would
/// have to address, and a popover needs neither: the button names it by id,
/// the browser draws it over the page, and a tap outside it or on the `×`
/// beside the name dismisses it. Still no script -- it is an attribute, the same kind of
/// thing `<details>` is.
///
/// The amounts keep their cents where the table above drops them: the rows
/// worth auditing are the interest postings and the import's, which carry
/// cents, and the total has to land on the balance rather than near it.
/// That is also why the allocations older than the ledgers reach are one
/// `Earlier` line ahead of the rest rather than left off.
fn history(goal: &Goal) -> String {
    let row = &goal.row;
    let note = row
        .note
        .as_deref()
        .map(|n| format!("<p class=\"note\">{}</p>", escape(n)))
        .unwrap_or_default();
    let body = if goal.history.is_empty() && goal.earlier.is_none() {
        "<p class=\"note\">No allocations yet.</p>".to_string()
    } else {
        let mut table = HISTORY_HEADER
            .iter()
            .map(|h| format!("<th>{h}</th>"))
            .collect::<String>();
        table = format!("<thead><tr>{table}</tr></thead>");
        if let Some(earlier) = &goal.earlier {
            table.push_str(&format!(
                "<tr><td>Earlier</td><td>{} allocations before {}</td>{}</tr>",
                earlier.count,
                earlier.before.format("%b %Y"),
                positional_money(Sense::Natural, earlier.cents),
            ));
        }
        for a in &goal.history {
            table.push_str(&format!(
                "<tr><td>{}</td><td>{}</td>{}</tr>",
                a.date,
                escape(&crate::description::render(
                    a.note.as_deref().unwrap_or_default()
                )),
                positional_money(Sense::Natural, a.cents),
            ));
        }
        let total = goal.history.iter().fold(
            goal.earlier.as_ref().map_or(Cents::ZERO, |e| e.cents),
            |sum, a| sum + a.cents,
        );
        table.push_str(&format!(
            "<tr class=\"tot\"><td>Total</td><td></td>{}</tr>",
            positional_money(Sense::Natural, total)
        ));
        format!("<table>{table}</table>")
    };
    format!(
        "<div class=\"history\" id=\"{}\" popover><header><h4>{}</h4>\
         <button type=\"button\" class=\"close\" popovertarget=\"{}\" \
         popovertargetaction=\"hide\" aria-label=\"Close\">\u{d7}</button></header>\
         <div class=\"rows\">{note}{body}</div></div>",
        history_id(row),
        escape(&row.name),
        history_id(row),
    )
}

/// The history screen's columns, in its order and wording.
const HISTORY_HEADER: [&str; 3] = ["Date", "Note", "Amount"];

/// A goal's percentage cell: on the funding ramp, or a plain `--`.
fn percent_cell(percent: Option<Percent>) -> Cell {
    match percent {
        Some(p) => Cell::Toned(format!("{}%", p.0), palette::hex(palette::percent(p))),
        None => Cell::Text("--".to_string()),
    }
}

/// A goal's history as data: `Earlier` ahead of the listed allocations, then
/// the total the balance beside the name is. Nothing allocated at all is
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

/// The Savings tab as data: per container a heading and its goals, each goal
/// pointing at its history in `histories`, and the Unallocated remainder
/// below.
#[cfg_attr(not(test), allow(dead_code))]
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

pub(super) fn sections(containers: &[Container]) -> String {
    containers.iter().map(section).collect()
}

#[cfg(test)]
mod tests {
    use super::super::fixture::{json, panel, row, snapshot};
    use super::super::page;
    use super::view;
    use crate::money::Cents;
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
        let mut s = snapshot(vec![row("Rainy Day", 500, 1_000)], 0);
        s.containers[0].rows[0].row.note = Some("Roof <first> & \u{2014}".to_string());
        let (_, histories) = built(&s);
        assert_eq!(histories[0]["note"], "Roof <first> & \u{2014}");
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
    use crate::palette;
    use crate::rate::Percent;

    /// One row at a given percentage, over the fixture's own figures: what is
    /// under test is the color the column carries, not the division that
    /// produced it.
    fn at(percent: Option<Percent>) -> crate::savings::Row {
        let mut row = row("Rainy Day", 500, 1_000);
        row.percent = percent;
        row
    }

    /// The screen's ramp and the page's are the same three stops, spelled
    /// `#rrggbb` here and handed to ratatui there -- a goal drawn green in
    /// the terminal and amber on the phone would be two answers to one
    /// question.
    #[test]
    fn the_percent_column_is_colored_by_the_funding_ramp() {
        let stops = [Percent::ZERO, Percent(50), Percent::ONE_HUNDRED];
        let page = page(&snapshot(stops.map(|p| at(Some(p))).to_vec(), 1_000));
        let panel = panel(&page, "savings");
        for p in stops {
            let cell = format!(
                "<td class=\"n\" style=\"color:{}\">{}%</td>",
                palette::hex(palette::percent(p)),
                p.0
            );
            assert!(panel.contains(&cell), "{p:?} is not on the ramp: {panel}");
        }
    }

    /// A goal with no positive target has no percentage to place on the ramp,
    /// so its `--` stays plain: an unfunded red there would report a goal with
    /// no target as a goal with no money.
    #[test]
    fn a_goal_with_no_percentage_is_left_uncolored() {
        let page = page(&snapshot(vec![at(None)], 1_000));
        let panel = panel(&page, "savings");
        assert!(
            panel.contains("<td class=\"n\">--</td>"),
            "the plain `--` vanished: {panel}"
        );
        for p in [Percent::ZERO, Percent(50), Percent::ONE_HUNDRED] {
            let hex = palette::hex(palette::percent(p));
            assert!(
                !panel.contains(&hex),
                "{hex} colored a row with no percentage: {panel}"
            );
        }
    }

    /// The screens' own precision, the Unallocated line included: the $0.23 a
    /// container sits at for months is noise beside figures in the
    /// thousands, and truncating it is what leaves the line saying nothing
    /// when there is nothing to say.
    #[test]
    fn every_figure_drops_its_cents_the_unallocated_line_included() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        assert!(page.contains(">500<"), "a savings figure grew cents");
        assert!(!page.contains("0.23"), "the remainder kept its cents");
    }

    /// The container the `Unallocated` row belongs to is named in its own
    /// color, in the heading above the table -- the screen's footer names the
    /// same container the same way, on the same line as the figure because a
    /// terminal has no heading to hang it off.
    #[test]
    fn the_container_heading_names_its_account_in_color() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        let teal = crate::db::account::AccountColor::Teal;
        assert!(
            panel.contains(&format!(
                "<h3><span class=\"{}\">Rainy Day</span></h3>",
                super::super::account_class(teal)
            )),
            "{panel}"
        );
        assert!(
            page.contains(&format!(
                ".{}{{color:{}}}",
                super::super::account_class(teal),
                palette::hex(palette::account(teal))
            )),
            "the class names no color"
        );
    }

    /// The marker the row used to carry is gone: it said "this container is
    /// within a dollar", which is now what the truncated figure itself says.
    #[test]
    fn the_unallocated_row_carries_no_marker() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        for marker in ["✓", "!"] {
            assert!(!panel.contains(marker), "{marker} survived: {panel}");
        }
    }

    /// A container allocated past what it holds reads red, the color every
    /// other negative figure on the page takes -- and a sub-dollar negative
    /// does not, because the truncation takes its sign along with its cents.
    #[test]
    fn a_negative_remainder_is_red_and_a_sub_dollar_one_is_not() {
        let red = "class=\"n neg\"";
        let mut over = snapshot(vec![], 1_000);
        over.containers[0].excess = crate::money::Cents(-250_000);
        let over = page(&over);
        let over = panel(&over, "savings");
        assert!(over.contains(red), "an overdrawn container was not red");
        assert!(over.contains("-2,500"), "{over}");

        let mut drift = snapshot(vec![], 1_000);
        drift.containers[0].excess = crate::money::Cents(-23);
        let drift = page(&drift);
        let drift = panel(&drift, "savings");
        assert!(!drift.contains(red), "sub-dollar drift drew red: {drift}");
        assert!(!drift.contains("-0"), "{drift}");
    }

    /// A goal's own balance drops its cents before its color is chosen, the
    /// same as the `Unallocated` row beneath it. The cents a container
    /// collects land on its goals too -- an interest share, an imported
    /// correction -- and a figure the page shows as `0` must not be marked as
    /// a debt.
    #[test]
    fn a_sub_dollar_negative_balance_is_neither_signed_nor_red() {
        let red = palette::hex(palette::NEGATIVE);
        let mut snapshot = snapshot(vec![row("Rainy Day", 0, 1_000)], 0);
        snapshot.containers[0].rows[0].row.current = crate::money::Cents(-23);
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        assert!(!panel.contains(&red), "sub-dollar drift drew red: {panel}");
        // `>-0<` rather than `-0`, which every goal date carries.
        assert!(!panel.contains(">-0<"), "{panel}");
        assert!(panel.contains("<td class=\"n\">0</td>"), "{panel}");
    }

    /// `$/Pay` is the screen's and not the page's: the sixth column put the
    /// table past a phone.
    #[test]
    fn the_per_paycheck_column_is_left_off_the_page() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let panel = panel(&page, "savings");
        assert!(!panel.contains("$/Pay"), "{panel}");
        assert!(!panel.contains(">25<"), "the per-paycheck figure: {panel}");
    }

    /// The goal's name is what opens its history, and the button and the
    /// popover agree on the id -- a mismatch renders a name that does nothing
    /// when tapped.
    #[test]
    fn a_goals_name_opens_its_own_history() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let panel = panel(&page, "savings");
        assert!(
            panel.contains(
                "<button type=\"button\" class=\"goal\" popovertarget=\"history-1\">Rainy Day</button>"
            ),
            "{panel}"
        );
        assert!(
            panel.contains(
                "<div class=\"history\" id=\"history-1\" popover><header><h4>Rainy Day</h4>"
            ),
            "{panel}"
        );
        assert!(
            panel.contains(
                "popovertarget=\"history-1\" popovertargetaction=\"hide\" \
                 aria-label=\"Close\">\u{d7}</button></header>"
            ),
            "the history cannot be closed: {panel}"
        );
    }

    /// The screen's columns, oldest first, to the cent, with the ledger's em
    /// dash for a row nobody said anything about -- and footed by what they
    /// come to, which is the balance the row shows.
    #[test]
    fn the_history_lists_every_allocation_and_foots_with_their_total() {
        let page = page(&snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000));
        let panel = panel(&page, "savings");
        let first = panel
            .find("<td>2026-07-10</td><td>Paycheck</td><td>300.00</td>")
            .expect("the noted row is missing");
        let second = panel
            .find("<td>2026-08-07</td><td>\u{2014}</td><td>200.00</td>")
            .expect("the unnoted row is missing");
        assert!(first < second, "the rows are not oldest first");
        assert!(
            panel.contains("<tr class=\"tot\"><td>Total</td><td></td><td>500.00</td></tr>"),
            "{panel}"
        );
    }

    /// What the ledgers would have dropped is one line ahead of the rest, and
    /// still counted in the total -- which is the balance beside the name,
    /// whatever the page leaves unlisted.
    #[test]
    fn allocations_before_the_cut_are_one_line_the_total_still_counts() {
        let mut snapshot = snapshot(vec![row("Rainy Day", 650, 1_000)], 1_000);
        snapshot.containers[0].rows[0].earlier = Some(crate::report::Earlier {
            before: crate::test_support::day(2026, 5, 1),
            count: 4,
            cents: crate::money::Cents::from_dollars(150),
        });
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        let earlier = panel
            .find("<td>Earlier</td><td>4 allocations before May 2026</td><td>150.00</td>")
            .expect("the earlier line is missing");
        let first = panel
            .find("<td>2026-07-10</td>")
            .expect("the listed rows are missing");
        assert!(earlier < first, "the earlier line is not first");
        assert!(
            panel.contains("<tr class=\"tot\"><td>Total</td><td></td><td>650.00</td></tr>"),
            "{panel}"
        );
    }

    /// A goal whose every allocation is older than the cut still has a
    /// history to show, and does not claim to have none.
    #[test]
    fn a_goal_with_only_earlier_allocations_does_not_say_it_has_none() {
        let mut snapshot = snapshot(vec![row("Rainy Day", 150, 1_000)], 1_000);
        let goal = &mut snapshot.containers[0].rows[0];
        goal.history.clear();
        goal.earlier = Some(crate::report::Earlier {
            before: crate::test_support::day(2026, 5, 1),
            count: 4,
            cents: crate::money::Cents::from_dollars(150),
        });
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        assert!(!panel.contains("No allocations yet."), "{panel}");
        assert!(
            panel.contains("<tr class=\"tot\"><td>Total</td><td></td><td>150.00</td></tr>"),
            "{panel}"
        );
    }

    /// A goal with nothing allocated says so rather than footing a zero, the
    /// way the screen's empty history does.
    #[test]
    fn a_goal_with_no_allocations_says_so_instead_of_footing_a_zero() {
        let mut snapshot = snapshot(vec![row("Rainy Day", 0, 1_000)], 1_000);
        snapshot.containers[0].rows[0].history.clear();
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        assert!(panel.contains("No allocations yet."), "{panel}");
        assert!(!panel.contains(">Total<"), "{panel}");
    }

    /// The goal's own note sits above the rows, as on the screen; a goal with
    /// none spends no line on it.
    #[test]
    fn a_goals_note_is_drawn_above_its_history() {
        let mut snapshot = snapshot(vec![row("Rainy Day", 500, 1_000)], 1_000);
        snapshot.containers[0].rows[0].row.note = Some("Roof <first>".to_string());
        let page = page(&snapshot);
        let panel = panel(&page, "savings");
        assert!(
            panel.contains(
                "<h4>Rainy Day</h4><button type=\"button\" class=\"close\" \
                 popovertarget=\"history-1\" popovertargetaction=\"hide\" \
                 aria-label=\"Close\">\u{d7}</button></header>\
                 <div class=\"rows\"><p class=\"note\">Roof &lt;first&gt;</p><table>"
            ),
            "{panel}"
        );
    }

    /// Unlike an empty Overview band, which is a group the owner does not
    /// have, a container holding an unallocated remainder is a fact worth
    /// seeing.
    #[test]
    fn a_container_with_no_open_goals_still_reports_its_unallocated_remainder() {
        let page = page(&snapshot(vec![], 1_000));
        let panel = panel(&page, "savings");
        assert!(panel.contains("Rainy Day"), "the container vanished");
        assert!(panel.contains("Unallocated"), "the remainder vanished");
    }

    /// A database with no containers holding goals is not a database with
    /// nothing to import -- the tab must still be there, over nothing.
    #[test]
    fn a_snapshot_with_no_containers_still_renders() {
        let mut without_containers = snapshot(vec![], 1_000);
        without_containers.containers.clear();
        let page = page(&without_containers);
        assert!(
            page.contains("<label for=\"savings\">Savings</label>"),
            "the Savings tab vanished"
        );
        assert!(
            panel(&page, "savings").is_empty(),
            "a container survived being cleared"
        );
    }
}
