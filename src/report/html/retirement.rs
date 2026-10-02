//! The Retirement tab: a spelling of [`crate::retirement`], the same model
//! screen 0 draws. A status wears an inline color off
//! [`palette::standing`], the way every other tab colors a figure.

use super::{account, escape, whole_money};
use crate::calc::retirement::{self, BAND_DASH, Band, Status};
use crate::money::Cents;
use crate::palette;
use crate::retirement::Retirement;

/// What a figure that cannot be stated draws as -- the screen's word too.
const ABSENT: &str = "--";

/// A figure in prose, with its `$` -- the cells beside it are
/// [`whole_money`]'s, which the column header already says are dollars.
fn dollars(cents: Cents) -> String {
    format!("${}", cents.trunc_to_dollar().to_whole_dollars())
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
            "<tr><td>Saved</td>{}<td class=\"n\">{}</td><td>{}</td>{}</tr>",
            whole_money(r.saved),
            escape(&multiple.to_string()),
            escape(&now.saved.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Saved</td>{}<td colspan=\"3\">Set a salary on the Retirement screen.</td></tr>",
            whole_money(r.saved)
        ),
    };
    let tax_free = match (r.tax_free_share(), r.tax_free_status()) {
        (Some(share), Some((s, gap))) => format!(
            "<tr><td>Tax-free</td>{}<td class=\"n\">{}%</td><td>{}</td>{}</tr>",
            whole_money(r.tax_free),
            share.tenth_percent(),
            escape(&now.tax_free.to_string()),
            status(s, gap)
        ),
        _ => format!(
            "<tr><td>Tax-free</td>{}<td colspan=\"3\">{ABSENT}</td></tr>",
            whole_money(r.tax_free)
        ),
    };
    format!(
        "<table><thead><tr><th></th><th>Balance</th><th>Now</th><th>Target</th><th></th></tr></thead>\
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
                "<tr><td>{}</td><td>{}</td><td class=\"n\">{}</td><td>{}</td></tr>",
                escape(&label),
                escape(&multiple),
                escape(&target),
                escape(&row.tax_free.to_string())
            )
        })
        .collect();
    format!(
        "<table><thead><tr><th>Milestone</th><th>× salary</th><th>Saved $</th><th>Tax-free</th></tr></thead>\
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
                whole_money(h.balance)
            )
        })
        .collect();
    format!(
        "<table><thead><tr><th>Account</th><th>Tax</th><th>Balance</th><th>Share</th></tr></thead>\
         <tbody>{rows}</tbody></table>"
    )
}

pub(super) fn panel(r: &Retirement) -> String {
    let mut title = String::from("Retirement");
    if let Some(age) = r.age {
        title.push_str(&format!(" · age {age}"));
    }
    if let Some(salary) = r.salary {
        title.push_str(&format!(" · salary {}", dollars(salary)));
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

    /// Nothing held tax-free against 37's 11.0% of $330,000.
    #[test]
    fn a_short_tax_free_share_names_the_dollars_to_move_across() {
        let html = panel(&retirement());
        assert!(html.contains("Short $36,300"), "{html}");
    }
}
