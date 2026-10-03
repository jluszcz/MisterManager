//! The Retirement tab: a spelling of [`crate::retirement`], the same model
//! screen 8 draws. A status wears an inline color off
//! [`palette::standing`], the way every other tab colors a figure.

use super::{account, escape, money};
use crate::calc::retirement::{self, BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::palette;
use crate::retirement::Retirement;

/// What a figure that cannot be stated draws as -- the screen's word too.
const ABSENT: &str = "--";

/// A whole-dollar figure with its `$`. Every dollar figure on this tab
/// carries one -- the balances, the gaps, the salary and the milestone
/// targets -- so a column of balances cannot read as a column of multiples
/// beside it.
fn dollars(cents: Cents) -> String {
    format!("${}", cents.trunc_to_dollar().to_whole_dollars())
}

/// [`dollars`] in a money cell, colored as every other money cell is.
fn dollar_cell(cents: Cents) -> String {
    money(dollars(cents), cents.trunc_to_dollar())
}

fn compact(cents: Cents) -> String {
    format!("${}", retirement::compact(cents))
}

fn dollar_band(band: Band<Cents>) -> String {
    match band.low == band.high {
        true => compact(band.low),
        false => format!("{}{BAND_DASH}{}", compact(band.low), compact(band.high)),
    }
}

fn status(status: Status, short_by: Option<Cents>) -> String {
    let text = match short_by {
        Some(gap) => format!("{} {}", status.label(), dollars(gap)),
        None => status.label().to_string(),
    };
    format!(
        "<td style=\"color:{}\">{}</td>",
        palette::hex(palette::standing(status)),
        escape(&text)
    )
}

/// The two standings, or the one thing standing between the owner and them.
fn standing(r: &Retirement) -> String {
    if r.held.is_empty() {
        return "<p>Mark investment accounts as Retirement on the Accounts screen.</p>".into();
    }
    let Some(now) = r.now() else {
        return "<p>Set a birth date on the Retirement screen.</p>".into();
    };
    let saved = match (r.multiple(), r.saved_status()) {
        (Some(multiple), Some((s, gap))) => format!(
            "<tr><td>Saved</td>{}<td class=\"n\">{}</td><td class=\"n\">{}</td>{}</tr>",
            dollar_cell(r.saved),
            escape(&multiple.to_string()),
            escape(&now.saved.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Saved</td>{}<td colspan=\"3\">Set a salary on the Retirement screen.</td></tr>",
            dollar_cell(r.saved)
        ),
    };
    let tax_free = match (r.tax_free_share(), r.tax_free_status()) {
        (Some(share), Some((s, gap))) => format!(
            "<tr><td>Tax-free</td>{}<td class=\"n\">{}%</td><td class=\"n\">{}</td>{}</tr>",
            dollar_cell(r.tax_free),
            share.tenth_percent(),
            escape(&now.tax_free.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Tax-free</td>{}<td colspan=\"3\">{ABSENT}</td></tr>",
            dollar_cell(r.tax_free)
        ),
    };
    format!(
        "<table><thead><tr><th></th><th class=\"n\">Balance</th><th class=\"n\">Now</th>\
         <th class=\"n\">Target</th><th></th></tr></thead>\
         <tbody>{saved}{tax_free}</tbody></table>"
    )
}

fn milestones(r: &Retirement) -> String {
    let rows: String = r
        .rows
        .iter()
        .map(|row| {
            let label = match row.now {
                true => format!("Now ({})", row.age),
                false => format!("By {}", row.age),
            };
            let multiple = match r.salary {
                Some(_) => row.saved.to_string(),
                None => ABSENT.to_string(),
            };
            let target = row
                .saved_dollars
                .map(dollar_band)
                .unwrap_or_else(|| ABSENT.to_string());
            format!(
                "<tr><td>{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td><td class=\"n\">{}</td></tr>",
                escape(&label),
                escape(&multiple),
                escape(&target),
                escape(&row.tax_free.to_string())
            )
        })
        .collect();
    format!(
        "<table class=\"bands\"><thead><tr><th>Milestone</th><th class=\"n\">× salary</th>\
         <th class=\"n\">Saved $</th><th class=\"n\">Tax-free</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

fn accounts(r: &Retirement) -> String {
    let rows: String = r
        .held
        .iter()
        .map(|h| {
            let share = h
                .share_of(r.saved)
                .map_or(ABSENT.to_string(), |s| format!("{s}%"));
            format!(
                "<tr><td>{}</td><td>{}</td>{}<td class=\"n\">{share}</td></tr>",
                account(&h.account),
                escape(&h.tax_label()),
                dollar_cell(h.balance)
            )
        })
        .collect();
    format!(
        "<table><thead><tr><th>Account</th><th>Tax</th><th class=\"n\">Balance</th>\
         <th class=\"n\">Share</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

pub(super) fn panel(r: &Retirement) -> String {
    let mut title = String::from("Retirement");
    if let Some(age) = r.age {
        title.push_str(&format!(" · Age {age}"));
    }
    if let Some(salary) = r.salary {
        title.push_str(&format!(" · Salary {}", dollars(salary)));
    }
    // The milestones stand on their own, as on the screen; only the account
    // list needs something marked.
    let held = match r.held.is_empty() {
        true => String::new(),
        false => accounts(r),
    };
    format!(
        "<h3>{}</h3>{}{}{held}",
        escape(&title),
        standing(r),
        milestones(r)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::retirement::Status;
    use crate::db::AccountId;
    use crate::db::account::TaxTreatment;
    use crate::retirement::{Held, rows};

    /// Thirty-seven, $330,000 saved against a $100,000 salary: short of
    /// 3.40× by $10,000, with nothing held tax-free.
    fn retirement() -> Retirement {
        let accounts = super::super::fixture::accounts();
        let saved = Cents::from_dollars(330_000);
        let salary = Cents::from_dollars(100_000);
        Retirement {
            age: Some(37),
            salary: Some(salary),
            saved,
            tax_free: Cents::ZERO,
            held: vec![Held {
                account: crate::account_label::Account::named(&accounts, AccountId(3)),
                treatment: TaxTreatment::TaxDeferred,
                tax_free: Cents::ZERO,
                tax_free_share: crate::rate::BasisPoints::ZERO,
                balance: saved,
            }],
            rows: rows(Some(37), Some(salary)),
        }
    }

    #[test]
    fn a_short_standing_wears_the_negative_color_and_names_the_gap() {
        let html = panel(&retirement());
        assert!(
            html.contains(&palette::hex(palette::standing(Status::Short))),
            "{html}"
        );
        assert!(html.contains("Short $10,000"), "{html}");
    }

    #[test]
    fn the_table_starts_at_now() {
        let html = panel(&retirement());
        assert!(html.contains("Now (37)"), "{html}");
        assert!(!html.contains("By 35"), "{html}");
        assert!(html.contains("4.00 – 4.50×"), "{html}");
        // 4.00–4.50× of $100,000.
        assert!(html.contains("$400K – $450K"), "{html}");
    }

    #[test]
    fn nothing_marked_says_where_to_mark_it() {
        let html = panel(&Retirement::default());
        assert!(
            html.contains("Mark investment accounts as Retirement"),
            "{html}"
        );
    }

    #[test]
    fn every_dollar_figure_carries_its_sign() {
        let html = panel(&retirement());
        assert!(html.contains(">$330,000</td>"), "{html}");
        assert!(!html.contains(">330,000</td>"), "{html}");
    }

    #[test]
    fn every_header_is_aligned_with_its_column() {
        let html = panel(&retirement());
        for table in super::super::fixture::tables(&html) {
            assert_eq!(
                super::super::fixture::misaligned_headers(table),
                Vec::<String>::new(),
                "{table}"
            );
        }
    }

    /// Nothing held tax-free against 11.0% of 37's $340,000 target.
    #[test]
    fn a_short_tax_free_standing_names_the_dollars_of_the_target() {
        let html = panel(&retirement());
        assert!(html.contains("Short $37,400"), "{html}");
    }
}
