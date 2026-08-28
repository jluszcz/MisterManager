//! The Overview screen. The figures it draws are `crate::overview`'s.

use crate::overview::{Balances, Line, Overview};
use crate::projection::Dates;

use super::{account_cell, amount, right_header};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Cell, Row, Table};

/// A data row. An account row is tinted by its account; a subtotal names a
/// band rather than an account, so it takes the plain foreground.
fn table_row(line: &Line, balances: Balances, style: Style) -> Row<'static> {
    let label = match &line.account {
        Some(account) => account_cell(account),
        None => Cell::from(line.label.clone()),
    };
    Row::new(vec![
        label,
        amount(balances.to_date),
        amount(balances.adhoc),
        amount(balances.month_end),
    ])
    .style(style)
}

/// The three balance columns' headers: the date each one is quoted at, and
/// nothing else.
///
/// The names live in the design and in the footer's key hints rather than in
/// the table. The date is the whole of what distinguishes these three columns
/// from each other; spelling out `To-Date`, `Paycheck-Eve` and `Month-End`
/// over them would repeat on every column what the footer already says once.
///
/// `scrubbed` marks the Paycheck-Eve date when it is not the date the paycheck
/// recurring transaction derives. It is the only column marked: To-Date cannot
/// scrub, and Month-End is derived from the date the `*` is already on, so a
/// second mark would say the same thing twice.
fn column_headers(dates: Dates, scrubbed: bool) -> [String; 3] {
    [
        dates.to_date.to_string(),
        format!("{}{}", dates.adhoc, if scrubbed { "*" } else { "" }),
        dates.month_end.to_string(),
    ]
}

/// A row of nothing, separating a subtotal from the section below it.
fn spacer() -> Row<'static> {
    Row::new(Vec::<Cell>::new())
}

/// A subtotal row: bold, and carrying no account, so it keeps the plain
/// foreground rather than borrowing a tint from whichever account it happens
/// to sit under. The weight is what separates it from the rows it sums --
/// they are the only unbolded rows on the screen.
fn subtotal_row(label: &str, balances: Balances) -> Row<'static> {
    table_row(
        &Line {
            account: None,
            label: label.to_string(),
            balances,
        },
        balances,
        Style::default().add_modifier(Modifier::BOLD),
    )
}

/// Three balance columns over the accounts, in bands.
///
/// Each section draws its accounts band by band, a subtotal under each band,
/// and then the section's own total. **A section holding one band draws no
/// band subtotals** — the band's subtotal and the section's total would be
/// the same number under two names, which is why credit shows a single
/// `Credit` line rather than one per level.
///
/// A subtotal is set apart from the accounts it sums by its weight alone --
/// every label starts in the same column.
///
/// Nothing else: the container reconciliation is the Savings screen's footer,
/// which is where the goals it is about are.
///
/// Each section is followed by a blank row — after the section total, not
/// after every band, so the bands read as one stack and Net stands apart from
/// both sections rather than as the end of the credit one. The blanks are
/// unconditional: a kind with no accounts leaves an airy screen, which is
/// cheaper than a rule about when a separator earns its line.
///
/// The header row is bold, carries only the three dates (see
/// [`column_headers`]), and holds a blank line beneath it so the dates read as
/// labels for the columns rather than as the first row of the cash block.
pub fn render(frame: &mut Frame, area: Rect, overview: &Overview, scrubbed: bool) {
    let bold = Style::default().add_modifier(Modifier::BOLD);
    let plain = Style::default();

    let mut rows: Vec<Row> = Vec::new();
    for section in [&overview.cash, &overview.credit] {
        for band in &section.bands {
            rows.extend(band.lines.iter().map(|l| table_row(l, l.balances, plain)));
            if section.breaks_down() {
                rows.push(subtotal_row(band.group.label(), band.total));
            }
        }
        rows.push(subtotal_row(section.kind.label(), section.total));
        rows.push(spacer());
    }
    rows.push(table_row(
        &Line {
            account: None,
            label: "Net".to_string(),
            balances: overview.net,
        },
        overview.net,
        bold,
    ));

    let [to_date, adhoc, month_end] = column_headers(overview.dates, scrubbed);
    let header = Row::new(vec![
        Cell::from("Account"),
        right_header(&to_date),
        right_header(&adhoc),
        right_header(&month_end),
    ])
    .style(bold)
    .bottom_margin(1);

    // Bare dates need only 11 columns, so the table sits well inside
    // `MIN_WIDTH`. Account takes whatever is left over.
    let widths = [
        Constraint::Min(18),
        Constraint::Length(16),
        Constraint::Length(16),
        Constraint::Length(16),
    ];
    frame.render_widget(
        Table::new(rows, widths)
            .header(header)
            .block(Block::bordered().title("Overview")),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::account::{Group, Kind};
    use crate::db::txn::{self, NewTxn};
    use crate::db::{self, AccountId, Db, account};
    use crate::test_support::day;
    use crate::tui::MIN_WIDTH;
    use chrono::NaiveDate;

    fn dates() -> Dates {
        Dates::new(day(2026, 8, 12), day(2026, 8, 27))
    }

    fn add(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64) {
        txn::insert(
            db,
            &NewTxn {
                date,
                cents: crate::money::Cents(cents),
                account_id,
                description: "row".to_string(),
                recurring_txn_id: None,
            },
        )
        .unwrap();
    }

    /// Inserts an account exactly as the import does: named by its code, in
    /// its kind's default band, appended to whatever that kind holds.
    fn imported(db: &Db, code: &str, kind: Kind) -> AccountId {
        let sort = account::list_by_kind(db, kind).unwrap().len() as i64;
        account::insert(db, code, code, kind, sort).unwrap()
    }

    /// The same, then named and banded the way the owner would on the
    /// Accounts screen. Order comes from the insert order, so a fixture
    /// stacks in the order it is written.
    fn placed(db: &Db, code: &str, name: &str, kind: Kind, group: Group) -> AccountId {
        let id = imported(db, code, kind);
        account::set_name(db, id, name).unwrap();
        account::set_group(db, id, group).unwrap();
        id
    }

    /// The trimmed text of each line inside the border, from the row under
    /// the top border down.
    fn draw(db: &Db, height: u16) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let overview = Overview::load(db, dates()).unwrap();
        let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, height)).unwrap();
        terminal
            .draw(|frame| {
                render(frame, frame.area(), &overview, false);
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        // Row 0 is the top border, and the first and last columns are the sides.
        (1..height - 1)
            .map(|y| {
                (1..MIN_WIDTH - 1)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    /// One cash account and one card. Index 0 is the header, 1 the blank
    /// under it, then Everyday, Cash, blank, Card One, Credit, blank, Net.
    fn drawn() -> Vec<String> {
        let db = db::open_in_memory().unwrap();
        let cash = placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        let card = placed(&db, "CC1", "Card One", Kind::Credit, Group::Credit);
        add(&db, cash, day(2026, 1, 1), 100_000);
        add(&db, card, day(2026, 1, 1), 7_000);
        draw(&db, 13).iter().map(|r| r.trim().to_string()).collect()
    }

    /// A full set of accounts, named, banded and ordered as the owner would.
    fn full_db() -> Db {
        let db = db::open_in_memory().unwrap();
        placed(&db, "CHK", "Everyday", Kind::Cash, Group::Checking);
        placed(&db, "SAV", "Rainy Day", Kind::Cash, Group::Savings);
        placed(&db, "BKR", "Brokerage", Kind::Cash, Group::Savings);
        for (code, name) in [
            ("CC1", "Card One"),
            ("CC2", "Card Two"),
            ("CC3", "Card Three"),
            ("CHK", "Everyday Card"),
        ] {
            placed(&db, code, name, Kind::Credit, Group::Credit);
        }
        db
    }

    /// The screen knows nothing about `--demo`: every figure it draws goes
    /// through `tui::amount`, and every account through `Account::render_with`,
    /// so scrambling either is one decision rather than one per screen. The
    /// rows and the bands are untouched -- what a demo hides is the money and
    /// the owner's own words for it, never the shape of the screen.
    #[cfg(feature = "demo")]
    #[test]
    fn a_demo_scrambles_the_balances_and_leaves_the_rows_standing() {
        crate::demo::install_with_salt(7);
        let rows = drawn();
        let text = rows.join("\n");
        assert!(!text.contains("1,000"), "the balance survived: {text:?}");
        assert!(
            !text.contains("70.00"),
            "the card balance survived: {text:?}"
        );
        assert!(
            text.contains(&crate::demo::figure(crate::money::Cents(100_000))),
            "no scrambled balance found: {text:?}"
        );
        assert!(
            text.contains(&crate::demo::figure(crate::money::Cents(-7_000))),
            "no scrambled card balance found: {text:?}"
        );
        assert!(!text.contains("Everyday"), "the account name survived");
        assert!(
            text.contains(&crate::demo::text("Everyday").to_string()),
            "no scrambled account name found: {text:?}"
        );
        assert!(text.contains("Net"), "the subtotals must stay");
    }

    /// So the dates read as labels for the columns below them rather than as
    /// the first row of the cash block.
    #[test]
    fn a_blank_line_follows_the_header() {
        let rows = drawn();
        assert_eq!(rows[1], "", "{:?}", &rows[..3]);
    }

    /// Each section total closes its own block, so the line under `Cash` and
    /// the line under `Credit` are blank and `Net` stands on its own.
    #[test]
    fn a_blank_line_follows_each_section_total() {
        let rows = drawn();
        assert!(rows[3].contains("Cash"), "{:?}", rows[3]);
        assert_eq!(rows[4], "", "the line under Cash must be blank");
        assert!(rows[6].contains("Credit"), "{:?}", rows[6]);
        assert_eq!(rows[7], "", "the line under Credit must be blank");
        assert!(rows[8].contains("Net"), "{:?}", rows[8]);
    }

    /// The order the screen stacks the accounts in is the owner's, not the
    /// workbook's column order, and every account shows its full name.
    #[test]
    fn the_rows_are_stacked_in_the_owners_order() {
        // The label is everything before the gap to the first amount column;
        // a run of three spaces cannot be inside a label.
        let labels: Vec<String> = draw(&full_db(), 18)
            .into_iter()
            .map(|r| match r.find("   ") {
                Some(gap) => r[..gap].to_string(),
                None => r,
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                "Account",
                "",
                "Everyday",
                "Checking",
                "Rainy Day",
                "Brokerage",
                "Savings",
                "Cash",
                "",
                "Card One",
                "Card Two",
                "Card Three",
                "Everyday Card",
                "Credit",
                "",
                "Net",
            ]
        );
    }

    /// Credit is one band, so its subtotal and the section total are the same
    /// number -- printing both would be the same line twice.
    #[test]
    fn a_section_of_one_band_prints_its_total_once() {
        let rows = draw(&full_db(), 18);
        let credits = rows.iter().filter(|r| r.contains("Credit")).count();
        assert_eq!(credits, 1, "{rows:?}");
    }

    /// The same suppression on the cash side. Credit exercises the rule with
    /// a section that can only ever hold one band; cash is the case where the
    /// section *could* break down and happens not to, so `Savings` must not
    /// appear above a `Cash` total that equals it.
    #[test]
    fn a_cash_section_of_one_band_prints_no_band_subtotal() {
        let db = db::open_in_memory().unwrap();
        placed(&db, "SAV", "Rainy Day", Kind::Cash, Group::Savings);
        placed(&db, "BKR", "Brokerage", Kind::Cash, Group::Savings);

        let rows = draw(&db, 12);

        assert!(
            rows.iter().any(|r| r.starts_with("Cash")),
            "no Cash total in {rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.starts_with("Savings")),
            "the lone band was drawn as well as the total, in {rows:?}"
        );
    }

    /// Every label starts in the same column: a subtotal is set apart by its
    /// weight, not by its position.
    #[test]
    fn no_row_label_is_indented() {
        let rows = draw(&full_db(), 18);
        for row in &rows {
            assert!(!row.starts_with(' '), "{row:?} is indented, in {rows:?}");
        }
    }

    /// The headers are the dates alone; the names they would carry are in
    /// the footer. Pinning the exact strings is what keeps a label from
    /// creeping back onto a right-aligned column, where it would sit over the
    /// padding rather than over its own figures.
    #[test]
    fn the_column_headers_are_the_bare_dates_each_column_is_quoted_at() {
        assert_eq!(
            column_headers(dates(), false),
            ["2026-08-12", "2026-08-27", "2026-09-01"]
        );
    }

    /// The marker is how an unsaved scrub is visible on the column itself
    /// rather than only in the footer.
    #[test]
    fn a_scrubbed_paycheck_eve_column_is_marked() {
        assert_eq!(column_headers(dates(), true)[1], "2026-08-27*");
    }

    #[test]
    fn only_the_scrubbable_column_is_ever_marked() {
        let headers = column_headers(dates(), true);
        assert_eq!(headers[0], "2026-08-12");
        assert_eq!(headers[2], "2026-09-01");
    }
}
