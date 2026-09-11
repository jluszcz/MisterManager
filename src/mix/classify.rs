//! Turns a filing's holdings into a fund's asset-class composition.
//!
//! A filing never states a holding's asset class -- `assetCat` names a
//! regulatory category, not "stock" or "bond", and a fund-of-funds' holdings
//! are other funds, whose `assetCat` (typically `EC`) says nothing about
//! what *they* hold. So the class is derived, on one of two paths forked by
//! how many holdings the filing lists.

use crate::db::fund_mix::{AssetClass, Slice};
use crate::rate::BasisPoints;

/// One row of an N-PORT filing's holdings list, past whatever ceremony the
/// SEC's field names carry -- `pct_val`, `assetCat` and `invCountry` are its
/// own spelling, kept here so a caller reading a raw filing does not have to
/// translate twice.
#[derive(Debug)]
pub struct RawHolding {
    pub name: String,
    pub title: String,
    pub cusip: String,
    pub pct_val: f64,
    pub asset_cat: String,
    pub inv_country: String,
}

/// Above this many holdings, a filing is a diversified book of securities
/// rather than a fund holding a handful of other funds -- measured against
/// real filings: two target-date funds listed 7 and 5 holdings, while three
/// direct index funds listed 3,546, 8,878 and 17,409. The two clusters are a
/// chasm, not a knife-edge, so a threshold anywhere in the gap tells them
/// apart; 25 sits well inside it.
pub const FUND_OF_FUNDS_MAX: usize = 25;

/// Keyword order matters: a name is tested for cash first, then bond, then
/// stock, because "International Bond Index Fund" matches both the bond and
/// the stock-ish `index` keyword, and the bond reading is the one that
/// matters -- a `STOCK` match only ever picks the domestic/international
/// variant once nothing narrower has already claimed the holding.
const CASH: [&str; 4] = ["liquidity", "money market", "short-term reserve", "cash"];
const BOND: [&str; 3] = ["bond", "treasury", "fixed income"];
const STOCK: [&str; 3] = ["index", "stock", "market"];
const INTL: [&str; 7] = [
    "international",
    "intl",
    "ex u.s.",
    "ex-u.s.",
    "global ex",
    "developed markets",
    "emerging markets",
];

/// A fund-of-funds holding's class, read off its own name and title
/// concatenated -- one measured family files an unreadable abbreviation as
/// `title` and full prose as `name`, another files the legal trust as `name`
/// and the fund as `title`, so reading only one field is a coin flip on
/// which family a filing came from.
fn classify_by_keyword(text: &str) -> AssetClass {
    let text = text.to_lowercase();
    if CASH.iter().any(|k| text.contains(k)) {
        return AssetClass::Cash;
    }
    let intl = INTL.iter().any(|k| text.contains(k));
    if BOND.iter().any(|k| text.contains(k)) {
        return if intl {
            AssetClass::IntlBond
        } else {
            AssetClass::UsBond
        };
    }
    if STOCK.iter().any(|k| text.contains(k)) {
        return if intl {
            AssetClass::IntlStock
        } else {
            AssetClass::UsStock
        };
    }
    AssetClass::Unclassified
}

/// A direct fund's holding's class, read off its own `assetCat` and
/// `invCountry` -- a security's `assetCat` is a fact the filing states
/// rather than a name to guess at, so this path has no keyword list.
/// `ABS-MBS` (mortgage-backed paper) is in the bond list on purpose: it was
/// 20.5% of one measured bond fund, and missing it would quietly lose a
/// fifth of that fund into `Unclassified`.
fn classify_by_category(asset_cat: &str, inv_country: &str) -> AssetClass {
    let intl = inv_country != "US";
    match asset_cat {
        "EC" | "EP" => {
            if intl {
                AssetClass::IntlStock
            } else {
                AssetClass::UsStock
            }
        }
        "DBT" | "ABS-MBS" | "ABS-CBDO" | "ABS-O" | "LON" => {
            if intl {
                AssetClass::IntlBond
            } else {
                AssetClass::UsBond
            }
        }
        "STIV" => AssetClass::Cash,
        _ => AssetClass::Unclassified,
    }
}

/// [`AssetClass::ALL`]'s own index of `class` -- a plain match rather than a
/// linear search, since every class in a filing's holdings is looked up once
/// per holding.
fn index(class: AssetClass) -> usize {
    match class {
        AssetClass::UsStock => 0,
        AssetClass::IntlStock => 1,
        AssetClass::UsBond => 2,
        AssetClass::IntlBond => 3,
        AssetClass::Cash => 4,
        AssetClass::Unclassified => 5,
    }
}

/// `n / d`, rounded to the nearest integer rather than truncated. Every
/// value this divides is non-negative -- a filing's `pct_val` is a share of
/// the fund, never a short position -- so the half-up bias this carries at
/// negative inputs never comes up.
fn round_div(n: i64, d: i64) -> i64 {
    (n + d / 2) / d
}

/// A filing's holdings, turned into a mix that foots to exactly
/// `BasisPoints::ONE`.
///
/// Forks on `holdings.len()`: at or under [`FUND_OF_FUNDS_MAX`] each holding
/// is another fund, classified by keyword over its name and title; above it,
/// holdings are securities, classified by `assetCat` and `invCountry`. A
/// holding classifying as nothing at all lands in `Unclassified` rather than
/// a neighbour -- that is what makes the heuristic safe to run unattended,
/// since a miss surfaces as a labelled row instead of money silently in the
/// wrong bucket.
pub fn classify(holdings: &[RawHolding]) -> Vec<Slice> {
    if holdings.is_empty() {
        // Not a filing the classifier ever fetched a holding for, so there
        // is nothing to divide by and nothing to derive -- and the
        // residual class already means "the filing didn't say", which is
        // exactly what an empty one is saying.
        return vec![Slice {
            class: AssetClass::Unclassified,
            weight: BasisPoints::ONE,
        }];
    }

    let fund_of_funds = holdings.len() <= FUND_OF_FUNDS_MAX;
    // Accumulated in hundredths of a basis point (1 bp = 100 of these):
    // rounding each holding to whole basis points before summing would let a
    // filing of two dozen small holdings drift the total away from 10,000
    // by more than the one-point remainder step below is meant to absorb.
    let mut totals = [0i64; AssetClass::ALL.len()];
    for holding in holdings {
        let class = if fund_of_funds {
            classify_by_keyword(&format!("{} {}", holding.name, holding.title))
        } else {
            classify_by_category(&holding.asset_cat, &holding.inv_country)
        };
        totals[index(class)] += (holding.pct_val * 10_000.0).round() as i64;
    }

    let mut weights: [i64; AssetClass::ALL.len()] =
        totals.map(|hundredths| round_div(hundredths, 100));
    let total: i64 = weights.iter().sum();
    // The remainder lands on the largest slice rather than being spread
    // across all of them: a one-point correction folded into the biggest
    // number is invisible, while spreading it moves several figures to fix
    // the one that was actually short.
    let (largest, _) = weights
        .iter()
        .enumerate()
        .max_by_key(|&(_, &w)| w)
        .expect("AssetClass::ALL is non-empty");
    weights[largest] += 10_000 - total;

    AssetClass::ALL
        .into_iter()
        .zip(weights)
        .filter(|&(_, weight)| weight != 0)
        .map(|(class, weight)| Slice {
            class,
            weight: BasisPoints(weight),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::fund_mix::AssetClass;
    use crate::rate::BasisPoints;

    /// A holding of an underlying fund: `assetCat` is `EC` and `invCountry` is
    /// `US` whatever the fund holds, which is why the name is what classifies.
    fn fund(name: &str, title: &str, pct: f64) -> RawHolding {
        RawHolding {
            name: name.into(),
            title: title.into(),
            cusip: "000000000".into(),
            pct_val: pct,
            asset_cat: "EC".into(),
            inv_country: "US".into(),
        }
    }

    fn security(asset_cat: &str, country: &str, pct: f64) -> RawHolding {
        RawHolding {
            name: "A Held Company".into(),
            title: String::new(),
            cusip: "000000000".into(),
            pct_val: pct,
            asset_cat: asset_cat.into(),
            inv_country: country.into(),
        }
    }

    fn weight(slices: &[crate::db::fund_mix::Slice], class: AssetClass) -> BasisPoints {
        slices
            .iter()
            .find(|s| s.class == class)
            .map(|s| s.weight)
            .unwrap_or(BasisPoints::ZERO)
    }

    #[test]
    fn a_fund_of_funds_is_classified_by_the_names_of_the_funds_it_holds() {
        let slices = classify(&[
            fund("Total Market Index Fund", "", 50.0),
            fund("International Stock Index Fund", "", 30.0),
            fund("Total Bond Index Fund", "", 15.0),
            fund("International Bond Index Fund", "", 5.0),
        ]);

        assert_eq!(weight(&slices, AssetClass::UsStock), BasisPoints(5_000));
        assert_eq!(weight(&slices, AssetClass::IntlStock), BasisPoints(3_000));
        assert_eq!(weight(&slices, AssetClass::UsBond), BasisPoints(1_500));
        assert_eq!(weight(&slices, AssetClass::IntlBond), BasisPoints(500));
    }

    /// One family files the readable name as `name` and another as `title`, so
    /// both are read and neither alone is enough.
    #[test]
    fn a_readable_name_in_either_field_classifies_the_holding() {
        let by_title = classify(&[fund("Some Street Trust", "Total Bond Index Fund", 100.0)]);
        assert_eq!(weight(&by_title, AssetClass::UsBond), BasisPoints(10_000));

        let by_name = classify(&[fund("Total Bond Index Fund", "TB II-INV", 100.0)]);
        assert_eq!(weight(&by_name, AssetClass::UsBond), BasisPoints(10_000));
    }

    #[test]
    fn a_name_naming_no_asset_class_lands_in_unclassified() {
        let slices = classify(&[fund("Overseas Growth Fund", "", 100.0)]);
        assert_eq!(
            weight(&slices, AssetClass::Unclassified),
            BasisPoints(10_000)
        );
    }

    /// `CASH` is checked first among the fund-of-funds keywords, so a name
    /// naming a cash-equivalent holding must land in `Cash` rather than
    /// falling through to `Unclassified` -- the one class the mandated test
    /// set never exercised, on either path.
    #[test]
    fn a_fund_of_funds_names_a_cash_holding_by_keyword() {
        let slices = classify(&[fund("Short-Term Reserve Fund", "", 100.0)]);
        assert_eq!(weight(&slices, AssetClass::Cash), BasisPoints(10_000));
    }

    /// A direct fund holds securities, and a fund share's `EC` would call the
    /// whole thing stock. Over the threshold, `assetCat` classifies instead.
    #[test]
    fn a_direct_fund_is_classified_by_asset_category_and_country() {
        let mut holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX + 1)
            .map(|_| security("EC", "US", 50.0 / (FUND_OF_FUNDS_MAX + 1) as f64))
            .collect();
        holdings.push(security("DBT", "DE", 50.0));

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::UsStock), BasisPoints(5_000));
        assert_eq!(weight(&slices, AssetClass::IntlBond), BasisPoints(5_000));
    }

    /// Mortgage-backed paper was a fifth of one measured bond fund. Dropping it
    /// would quietly lose that fifth.
    #[test]
    fn mortgage_backed_paper_counts_as_a_bond() {
        let holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX + 1)
            .map(|_| security("ABS-MBS", "US", 100.0 / (FUND_OF_FUNDS_MAX + 1) as f64))
            .collect();

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::UsBond), BasisPoints(10_000));
    }

    /// `STIV` is the direct-fund path's own `Cash` category -- the
    /// counterpart to the fund-of-funds keyword test above, since neither
    /// path's mandated test set exercised `Cash` on its own.
    #[test]
    fn a_direct_funds_short_term_investment_vehicle_counts_as_cash() {
        let holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX + 1)
            .map(|_| security("STIV", "US", 100.0 / (FUND_OF_FUNDS_MAX + 1) as f64))
            .collect();

        let slices = classify(&holdings);

        assert_eq!(weight(&slices, AssetClass::Cash), BasisPoints(10_000));
    }

    #[test]
    fn an_unknown_asset_category_lands_in_unclassified_rather_than_a_neighbour() {
        let mut holdings: Vec<RawHolding> = (0..FUND_OF_FUNDS_MAX)
            .map(|_| security("EC", "US", 90.0 / FUND_OF_FUNDS_MAX as f64))
            .collect();
        holdings.push(security("WAT", "US", 10.0));

        let slices = classify(&holdings);

        assert_eq!(
            weight(&slices, AssetClass::Unclassified),
            BasisPoints(1_000)
        );
    }

    #[test]
    fn the_slices_always_foot_to_one_hundred_percent() {
        // Three thirds do not divide 10,000 evenly; the remainder goes to the
        // largest bucket rather than leaving the row short.
        let slices = classify(&[
            fund("Total Market Index Fund", "", 33.333),
            fund("International Stock Index Fund", "", 33.333),
            fund("Total Bond Index Fund", "", 33.334),
        ]);

        let total: i64 = slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, 10_000, "the slices did not foot to 100%");
    }

    #[test]
    fn a_filing_with_no_holdings_foots_to_unclassified_rather_than_panicking() {
        let slices = classify(&[]);
        assert_eq!(
            weight(&slices, AssetClass::Unclassified),
            BasisPoints(10_000)
        );
    }
}
