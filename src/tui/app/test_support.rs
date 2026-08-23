//! Helpers shared by the screen modules' tests.
//!
//! The screens' tests split along with the screens, and what several of
//! them build -- the fixture app, the keystroke helpers, the accessors
//! that name the open modal -- is one seam rather than one copy per
//! module.

use super::*;
use crate::db;
use crate::db::account::Group;
use crate::db::goal;
use crate::db::txn::NewTxn;
use crate::gate::Gate;
use crate::money::Cents;
use crate::plan_line::{Destination, Line};
use crate::test_support::day;
use crate::tui::MIN_WIDTH;
use crate::tui::form::{TxnField, TxnForm};
use crate::tui::picker::Picker;
use crate::tui::planning::{Target, TransferConfirm};
use crate::tui::worksheet::Worksheet;

pub(super) fn today() -> NaiveDate {
    day(2026, 8, 15)
}

pub(super) fn write(db: &Db, account_id: AccountId, date: NaiveDate, cents: i64, what: &str) {
    txn::insert(
        db,
        &NewTxn {
            date,
            cents: Cents(cents),
            account_id,
            description: what.to_string(),
            recurring_txn_id: None,
        },
    )
    .unwrap();
}

/// Two cash accounts, two cards, and a handful of rows in the month the
/// ledgers open on, including one dated after today. Enough for the
/// cursor, the `Tab` filter, and autocomplete to have something to act
/// on.
pub(super) fn app() -> App {
    let db = db::open_in_memory().unwrap();
    let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
    account::set_group(&db, checking, Group::Checking).unwrap();
    let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
    let card_one = account::insert(&db, "CC1", "Card One", Kind::Credit, 0).unwrap();
    let card_two = account::insert(&db, "CC2", "Card Two", Kind::Credit, 1).unwrap();
    write(&db, checking, day(2026, 8, 1), 100_000, "Paycheck");
    write(&db, checking, day(2026, 8, 10), -5_000, "Whole Foods");
    write(&db, savings, day(2026, 8, 12), 20_000, "Transfer");
    write(&db, checking, day(2026, 8, 20), -120_000, "Rent");
    write(&db, card_one, day(2026, 8, 11), 1_499, "Movies");
    write(&db, card_two, day(2026, 8, 13), 2_599, "Batteries");
    let vacation = goal::insert(
        &db,
        &goal::NewGoal {
            name: "Vacation 2027".to_string(),
            container_account_id: savings,
            base_cents: Cents(1_500_000),
            goal_date: Some(day(2027, 1, 1)),
            recurring_goal_id: None,
            interest_eligible: true,
            sort: 0,
            taxed: false,
        },
    )
    .unwrap();
    goal::insert_allocation(&db, vacation, day(2026, 8, 1), Cents(1_000_000), None, None).unwrap();
    let couch = goal::insert(
        &db,
        &goal::NewGoal {
            name: "Couch".to_string(),
            container_account_id: savings,
            base_cents: Cents(100_000),
            goal_date: None,
            recurring_goal_id: None,
            interest_eligible: true,
            sort: 1,
            taxed: false,
        },
    )
    .unwrap();
    goal::insert_allocation(&db, couch, day(2026, 8, 1), Cents(25_000), None, None).unwrap();
    App::new(db, today()).unwrap()
}

pub(super) fn savings_names(app: &App) -> Vec<String> {
    app.savings.rows().iter().map(|r| r.name.clone()).collect()
}

/// The footer as text. It is a `Line` because the `/` box draws a caret
/// into it, and every assertion here is about the words.
pub(super) fn footer(app: &App) -> String {
    app.footer()
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect()
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
}

/// A `Ctrl` combination -- the editing keys, and nothing else in the app.
pub(super) fn ctrl_press(app: &mut App, c: char) {
    app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

pub(super) fn type_str(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

pub(super) fn savings_favorites(app: &App) -> Vec<bool> {
    app.savings.rows().iter().map(|r| r.favorite).collect()
}

/// Tab from the field the form opens on to `field`.
///
/// The likely failure is not a closed form but an open autocomplete
/// popup: it answers `Tab` before the form does, so a description
/// matching a row already written leaves the focus where it was through
/// every press. Say which, and on which field, rather than blaming a
/// form that is sitting right there.
pub(super) fn focus(app: &mut App, field: TxnField) {
    for _ in 0..TxnField::ORDER.len() {
        match &app.modal {
            Some(Modal::Txn(form)) if form.focus == field => return,
            _ => press(app, KeyCode::Tab),
        }
    }
    let state = match &app.modal {
        Some(Modal::Txn(form)) => format!("the focus stuck on {}", form.focus.label()),
        Some(_) => "another modal is open".to_string(),
        None => "no transaction form is open".to_string(),
    };
    panic!(
        "{} never took focus: {state}, with {} suggestions showing",
        field.label(),
        app.popup.visible()
    );
}

/// The open modal, as the variant a test expects it to be.
///
/// One accessor per variant a test reaches for, rather than a `let else`
/// at each site: what a failure says is then the same sentence wherever it
/// fires, and it names the modal that *is* open -- which is what a test
/// pressing the wrong key needs told, and what twenty hand-written
/// panics each phrased differently were not saying.
macro_rules! open_modal {
    ($name:ident, $variant:ident, $ty:ty, $what:literal) => {
        pub(super) fn $name(app: &App) -> &$ty {
            match &app.modal {
                Some(Modal::$variant(it)) => it,
                other => panic!(
                    "no {} is open: {}, with {:?} on the status line",
                    $what,
                    match other {
                        Some(_) => "another modal is",
                        None => "nothing is",
                    },
                    app.status
                ),
            }
        }
    };
}

open_modal!(form, Txn, TxnForm, "transaction form");
open_modal!(worksheet, Worksheet, Worksheet, "worksheet");
open_modal!(
    plan_transfers,
    PlanTransfers,
    TransferConfirm,
    "transfer confirmation"
);
open_modal!(picker, Picker, Picker, "picker");
open_modal!(
    destination,
    Destination,
    destination::Chooser,
    "destination chooser"
);

pub(super) fn descriptions(ledger: &Ledger) -> Vec<&str> {
    ledger
        .rows()
        .iter()
        .map(|t| t.description.as_str())
        .collect()
}

/// Everything the screen draws, as one string per test to search.
pub(super) fn drawn(app: &mut App) -> String {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    let mut terminal = Terminal::new(TestBackend::new(MIN_WIDTH, 24)).unwrap();
    terminal.draw(|frame| app.render(frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    (0..24)
        .map(|y| {
            (0..MIN_WIDTH)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// Two housing bills and one other, so both waterfall lines have
/// something in them and a transposition is visible.
pub(super) fn with_bills(db: &Db) {
    use crate::db::bill::{self, Category, NewBill};

    let bill = |label: &str, dollars: i64, category, sort| NewBill {
        label: label.to_string(),
        cents: Cents::from_dollars(dollars),
        category,
        sort,
    };
    bill::insert(db, &bill("Mortgage", 1_200, Category::Housing, 0)).unwrap();
    bill::insert(db, &bill("HOA", 300, Category::Housing, 1)).unwrap();
    bill::insert(db, &bill("Coworking", 1_000, Category::Other, 0)).unwrap();
}

/// The Rainy Day and Brokerage containers, the six lines with a goal-backed
/// destination, and the settings that point each at its goal -- the same
/// shape `transfer::tests::configured` builds, so the destination block
/// resolves rather than falling back to withdrawals.
///
/// The two spare unclaimed goals that divide the Goals plug live in
/// Brokerage rather than Rainy Day, unlike `transfer::tests::configured`:
/// `nulling_a_lines_destination_key_moves_it_out_of_its_group_on_screen`
/// clears Future Housing's key, which un-claims a Brokerage goal. If the
/// plug's goals sat in Rainy Day, that would leave two containers each holding
/// an unclaimed goal and `spread_container` would refuse to pick one.
pub(super) fn planning_app() -> App {
    let db = db::open_in_memory().unwrap();
    let checking = account::insert(&db, "CHK", "Everyday", Kind::Cash, 0).unwrap();
    account::set_group(&db, checking, Group::Checking).unwrap();
    let savings = account::insert(&db, "SAV", "Rainy Day", Kind::Cash, 1).unwrap();
    let brokerage = account::insert(&db, "BKR", "Brokerage", Kind::Cash, 2).unwrap();
    // A few cents of drift off the round paycheck so `Excess (Actual)`
    // has something for `floor_to_dollar` to floor.
    write(&db, checking, day(2026, 8, 1), 5_000_007, "Paycheck");
    with_bills(&db);

    let add_goal = |container: AccountId, name: &str| -> GoalId {
        goal::insert(
            &db,
            &goal::NewGoal {
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
    let bill_payments = add_goal(savings, "Bill Payments");
    let housing = add_goal(savings, "Housing");
    let roth = add_goal(savings, "Roth IRA");
    let down_payment = add_goal(brokerage, "Home Down Payment");
    let mom_and_dad = add_goal(brokerage, "Mom & Dad");
    let emergency = add_goal(brokerage, "Emergency Savings");
    add_goal(brokerage, "Lego");
    add_goal(brokerage, "Dropbox");
    // Fund the two gate-backed goals fully so neither gate fires: this
    // fixture exists to test where money lands, not to exercise the
    // gates.
    for id in [roth, emergency] {
        goal::insert_allocation(
            &db,
            id,
            day(2026, 8, 1),
            Cents::from_dollars(1_000),
            None,
            None,
        )
        .unwrap();
    }

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

    App::new(db, today()).unwrap()
}

/// `planning_app` plus one Everyday row three days after today, so the three
/// days between the derived Paycheck-Eve date and the scrubbed one hold a
/// balance change big enough to move the whole waterfall.
pub(super) fn planning_app_with_a_row_after_today() -> App {
    let mut app = planning_app();
    let checking = account::by_code(&app.db, "CHK", Kind::Cash)
        .unwrap()
        .unwrap()
        .id;
    write(&app.db, checking, day(2026, 8, 18), -1_000_000, "Rent");
    app.reload().unwrap();
    app
}

/// Moves the Planning cursor onto the first bill row and answers which one
/// it landed on.
pub(super) fn select_first_bill(app: &mut App) -> crate::db::bill::Bill {
    while !matches!(app.planning.selected_target(), Some(Target::Bill(_))) {
        press(app, KeyCode::Down);
    }
    let Some(Target::Bill(id)) = app.planning.selected_target() else {
        unreachable!()
    };
    crate::db::bill::get(&app.db, id).unwrap()
}

pub(super) fn destination_key(line: Line) -> crate::db::setting::Key<GoalId> {
    match line.destination() {
        Destination::Goal(key) => key,
        other => panic!("{line:?} resolves to {other:?}"),
    }
}

/// The panel is open, and on the topic named.
pub(super) fn open_on(app: &App, topic: Topic) -> bool {
    app.help.as_ref().is_some_and(|h| h.topic() == topic)
}

pub(super) fn entries(app: &App) -> Vec<String> {
    let picker = picker(app);
    picker.entries().iter().map(|e| e.name.clone()).collect()
}

pub(super) fn chosen(app: &App) -> Vec<String> {
    let picker = picker(app);
    picker.chosen().iter().map(|e| e.name.clone()).collect()
}
