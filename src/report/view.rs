//! What the report's script is handed: every tab as data, every decision
//! already made.
//!
//! The page is a shell plus a JSON payload of these types, and a script that
//! only lays them out. Whether a figure is negative, which color an account
//! wears and how wide a bar's segment is are all settled here, in Rust, from
//! the same `palette` and `account_label` the screens read -- the script
//! never re-derives one, so the two media cannot come to disagree.

use crate::account_label::Account;
use crate::db::account::AccountColor;
use crate::money::Cents;
use crate::palette::{self, Sense};
use serde::Serialize;
use std::collections::HashMap;

/// One table cell, in the fewest bytes the payload can spell it: bare text, a
/// `[text, tone]` pair, or an index into [`Page::accounts`]. `tone` is a class
/// name (`neg`, `pos`) or a `#rrggbb` the script sets directly.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Cell {
    Text(String),
    Toned(String, String),
    Account(usize),
}

#[derive(Debug, Serialize)]
pub struct Row {
    pub c: Vec<Cell>,
    /// Row classes, space-separated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub k: Option<String>,
    /// Index into [`Page::histories`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h: Option<usize>,
    /// Planning depth, from 2; the script spends it as indentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<u8>,
    /// Savings meter, `0..=100`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "t")]
pub enum Block {
    /// An `h3`; an account's heading is [`Cell::Account`].
    Heading {
        text: Cell,
    },
    /// An `h4`.
    Subheading {
        text: String,
    },
    Note {
        text: String,
    },
    Table {
        class: &'static str,
        head: Vec<String>,
        rows: Vec<Row>,
    },
    /// A ledger's month filter and the months under it. `opens` is the key of
    /// the month shown first, `None` meaning All months.
    Ledger {
        months: Vec<Month>,
        opens: Option<String>,
        empty: Option<String>,
    },
    /// Segments are `(width %, palette::CLASSES index)`; the legend pairs a
    /// label with the same index.
    Bar {
        segments: Vec<(String, usize)>,
        legend: Vec<(String, usize)>,
    },
    /// `svg` is markup built in Rust. A legend entry's color is an account
    /// ([`Cell::Account`]) or, with `None`, the Total line.
    Chart {
        title: String,
        svg: String,
        legend: Vec<(Cell, Option<usize>)>,
    },
}

/// A ledger month. A row's cells are the day (`03`), the account, the
/// description and the amount.
#[derive(Debug, Serialize)]
pub struct Month {
    pub key: String,
    pub label: String,
    pub rows: Vec<Row>,
}

#[derive(Debug, Serialize)]
pub struct History {
    pub name: String,
    pub note: Option<String>,
    pub rows: Vec<Row>,
}

#[derive(Debug, Serialize)]
pub struct TabView {
    pub id: &'static str,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Serialize)]
pub struct Page {
    /// `(name, "a-<AccountColor>")`, each account once however often it is
    /// drawn.
    pub accounts: Vec<(String, String)>,
    pub tabs: Vec<TabView>,
    pub histories: Vec<History>,
}

/// Builds [`Page::accounts`]: an account is stored once and every cell
/// naming it carries the index.
#[derive(Debug, Default)]
pub struct Accounts {
    entries: Vec<(String, String)>,
    seen: HashMap<(String, String), usize>,
}

impl Accounts {
    pub fn new() -> Self {
        Self::default()
    }

    /// Keyed on the text and color `render_with` resolves rather than the
    /// account's id, so two rows drawn identically are one entry. Reading
    /// the text through `render_with` keeps this the same route every other
    /// sink takes to it.
    pub fn cell(&mut self, account: &Account) -> Cell {
        let (text, color) = account.render_with(|text, color| (text.to_string(), color));
        let class = format!("a-{}", color.as_str());
        let entries = &mut self.entries;
        let index = *self
            .seen
            .entry((text.clone(), class.clone()))
            .or_insert_with(|| {
                entries.push((text, class));
                entries.len() - 1
            });
        Cell::Account(index)
    }

    pub fn into_vec(self) -> Vec<(String, String)> {
        self.entries
    }
}

/// A money cell in whichever [`Sense`] the column runs; below zero is the one
/// thing that tones a figure, and [`palette::amount`] is the decision.
pub fn money(sense: Sense, text: String, cents: Cents) -> Cell {
    match palette::amount(sense, cents) {
        Some(rgb) if rgb == palette::NEGATIVE => Cell::Toned(text, "neg".to_string()),
        Some(_) => Cell::Toned(text, "pos".to_string()),
        None => Cell::Text(text),
    }
}

/// Whole dollars, the cents dropped *before* the tone is chosen: `-$0.40`
/// shows as `$0` and must not be red.
pub fn whole(cents: Cents) -> Cell {
    let truncated = cents.trunc_to_dollar();
    money(Sense::Natural, truncated.to_whole_dollars(), truncated)
}

/// The custom properties and classes the script's output is colored with.
///
/// `--total` is black on a light ground and has to turn over on a dark one;
/// `--fg` is the shared shell's text color, so Total follows it.
pub fn tokens_css() -> String {
    let mut root = String::new();
    let mut classes = String::new();
    for &c in AccountColor::ALL.iter() {
        let name = c.as_str();
        root.push_str(&format!(
            "--a-{name}:{};",
            palette::hex(palette::account(c))
        ));
        classes.push_str(&format!(".a-{name}{{color:var(--a-{name})}}"));
    }
    root.push_str(&format!(
        "--neg:{};--pos:{};",
        palette::hex(palette::NEGATIVE),
        palette::hex(palette::POSITIVE)
    ));
    for (i, rgb) in palette::CLASSES.iter().enumerate() {
        root.push_str(&format!("--c{i}:{};", palette::hex(*rgb)));
    }
    root.push_str(&format!("--total:{};", palette::hex(palette::TOTAL)));
    format!(
        ":root{{{root}}}{classes}.neg{{color:var(--neg)}}.pos{{color:var(--pos)}}\
         @media (prefers-color-scheme:dark){{:root{{--total:var(--fg)}}}}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::AccountId;
    use crate::db::account::Kind;
    use serde_json::json;

    #[test]
    fn a_cell_serializes_as_text_pair_or_index() {
        assert_eq!(json!(Cell::Text("x".into())), json!("x"));
        assert_eq!(
            json!(Cell::Toned("x".into(), "neg".into())),
            json!(["x", "neg"])
        );
        assert_eq!(json!(Cell::Account(2)), json!(2));
    }

    #[test]
    fn a_row_omits_every_unset_field() {
        let row = Row {
            c: vec![],
            k: None,
            h: None,
            d: None,
            m: None,
        };
        assert_eq!(json!(row), json!({"c": []}));
    }

    #[test]
    fn one_account_is_one_entry_however_often_it_appears() {
        let account = Account::held(AccountId(1), "Everyday", None);
        let mut accounts = Accounts::new();
        let (a, b) = (accounts.cell(&account), accounts.cell(&account));
        assert_eq!(json!(a), json!(b));
        assert_eq!(accounts.into_vec().len(), 1);
    }

    #[test]
    fn money_below_zero_is_negative_and_a_credit_payment_is_positive() {
        let tone = |cell: Cell| match cell {
            Cell::Toned(_, tone) => tone,
            other => panic!("expected a toned cell, got {other:?}"),
        };
        assert_eq!(tone(money(Sense::Natural, "x".into(), Cents(-1))), "neg");
        assert_eq!(
            tone(money(Sense::of_ledger(Kind::Credit), "x".into(), Cents(-1))),
            "pos"
        );
    }

    #[test]
    fn cents_dropped_by_a_whole_dollar_cell_do_not_tone_it() {
        assert!(matches!(whole(Cents(-40)), Cell::Text(_)));
    }

    #[test]
    fn tokens_name_every_account_color() {
        let css = tokens_css();
        for c in AccountColor::ALL {
            let name = c.as_str();
            assert!(css.contains(&format!(".a-{name}{{")), "{name}");
            assert!(css.contains(&format!("--a-{name}:")), "{name}");
        }
    }
}
