//! What the portfolio holds by asset class, against what the age rule says it
//! should hold.
//!
//! A peer of `overview`, `savings` and `plan_rows`, and in neither medium:
//! the Funds screen spends these rows in a terminal and the report's Funds
//! tab spells the same ones as HTML, so the apportioning, the row order and
//! the combining of the two bond classes are stated once here rather than
//! twice over.
//!
//! The look-through is a share of what it *covers*. A holding whose ticker
//! has no `fund_mix` row on record is outside the denominator entirely --
//! folding it in would leave every class short by the same unnamed fraction,
//! which reads as an allocation rather than as a gap. What such a holding
//! costs the summary is said where the reader can act on it: the `—` on its
//! own row, and [`Allocation::coverage`] in the panel's title. A holding whose
//! mix exists but does not *foot* is a different case and stays inside: what
//! that filing failed to place lands in `Unclassified`, which is
//! [`apportion`]'s to argue.

use crate::calc::fund::Targets;
use crate::db::fund_mix::{AssetClass, Slice};
use crate::money::Cents;
use crate::rate::BasisPoints;

/// The classes the age rule says nothing about, in the order the summary
/// lists them under the three it does.
///
/// Here rather than at a screen for the reason [`TargetClass::ALL`] is: two
/// sinks drawing these rows in two orders would be two answers to one
/// question about one portfolio.
pub const UNTARGETED: [AssetClass; 2] = [AssetClass::Cash, AssetClass::Unclassified];

/// The four classes the summary's mix bar splits the portfolio into, in the
/// order it draws them: the equities, then the bonds the target rows combine.
///
/// **The bar is the one thing in the summary that splits the bonds**, which
/// is what it is for -- the age rule produces one bond number, so the row
/// beside it cannot say whether the share is domestic or foreign. What the
/// four leave over is cash, whatever the classifier could not place, and
/// whatever no filing placed at all; each sink draws that remainder as its
/// own medium's "nothing here", since a bar reading as though these four were
/// the whole portfolio would be the one way it could lie.
pub const BAR_CLASSES: [AssetClass; 4] = [
    AssetClass::UsStock,
    AssetClass::IntlStock,
    AssetClass::UsBond,
    AssetClass::IntlBond,
];

/// The portfolio's composition, and how much of it the composition is of.
///
/// `slices` foots to [`BasisPoints::ONE`] whenever there is anything to
/// apportion, and is empty when there is not -- a portfolio nobody has
/// fetched a mix for has no composition, which is a different statement from
/// one that is all cash.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Allocation {
    /// One entry per [`AssetClass`], in `AssetClass::ALL` order, less a zero
    /// `Unclassified`.
    ///
    /// The other five keep their zeroes: they are the vocabulary the targets
    /// and the bar are stated in, so a zero there is an answer. `Unclassified`
    /// is the residual -- what the classifier could not place, plus what a
    /// filing did not place at all -- and a row reporting that nothing went
    /// unplaced is a row nobody reads.
    pub slices: Vec<Slice>,
    /// Holdings whose ticker has a mix on record: the summary's denominator.
    pub covered: usize,
    /// Every holding the summary was asked about, covered or not.
    pub holdings: usize,
}

impl Allocation {
    /// What the summary covers, for a title to name -- `None` when it covers
    /// everything it was asked about.
    ///
    /// Drawn only when it has something to say, the way the two Planning
    /// transfer footers are: a title repeating "all of them" on every
    /// complete portfolio is a title nobody finishes reading, and the one
    /// time the count matters is the one time it is short.
    pub fn coverage(&self) -> Option<String> {
        (self.covered < self.holdings)
            .then(|| format!("{} of {} holdings", self.covered, self.holdings))
    }
}

/// One class's share of `slices`, zero for a class they do not name.
///
/// Absent and zero are the same answer *here*, and only here: a class no
/// filing mentions is a class the portfolio holds none of. The distinction
/// that does matter -- a fund nobody has fetched at all -- is made a level
/// up, by leaving that holding out of the apportioning entirely.
pub fn weight(slices: &[Slice], class: AssetClass) -> BasisPoints {
    slices
        .iter()
        .find(|s| s.class == class)
        .map_or(BasisPoints::ZERO, |s| s.weight)
}

/// Each holding's balance apportioned by its fund's composition, summed by
/// class, as basis points of the covered balance.
///
/// `None` for a holding's mix is a fund nobody has fetched: it counts toward
/// [`Allocation::holdings`] and toward nothing else.
///
/// **What a filing does not place lands in `Unclassified`.** Nothing guards a
/// `fund_mix` row's own footing, so a composition coming to 99% is reachable
/// rather than theoretical -- and the two other answers both lose the fact.
/// Dividing by what was placed instead of by the balance renormalises the gap
/// away across the classes that *were* placed, which says nothing; leaving it
/// out would foot to 99% in a table whose one unreadable state is a total
/// that does not foot. `Unclassified` is the class that exists for exactly
/// this, drawn only when non-zero, so routing the gap there foots *and*
/// surfaces it as the labelled row a miss is supposed to show up as. A filing
/// that over-foots surfaces the same way, as a negative one.
///
/// Truncating each share and dividing the leftover by largest remainder is
/// [`crate::calc::interest::pro_rata`]'s method and is here for its reason:
/// the shares have to foot exactly, and dumping the whole leftover on the
/// largest of them can exceed a share when several round up at once. Ties
/// break on `AssetClass::ALL` order, so one portfolio has one summary.
pub fn apportion(holdings: &[(Cents, Option<&[Slice]>)]) -> Allocation {
    let covered = holdings.iter().filter(|(_, mix)| mix.is_some()).count();
    let whole = i128::from(BasisPoints::ONE.0);
    let mut cents = [0i128; AssetClass::ALL.len()];
    let mut basis = 0i128;
    for (balance, mix) in holdings {
        let Some(mix) = mix else { continue };
        basis += i128::from(balance.0);
        for slice in *mix {
            cents[index_of(slice.class)] +=
                i128::from(balance.0) * i128::from(slice.weight.0) / whole;
        }
        // The gap in basis points rather than in cents, so the per-slice
        // truncation above -- a few cents at most, and genuinely nobody's
        // miss -- stays dust for the largest remainder to absorb instead of
        // drawing an `Unclassified` row reading 0.01%.
        let unplaced = whole - mix.iter().map(|s| i128::from(s.weight.0)).sum::<i128>();
        cents[index_of(AssetClass::Unclassified)] += i128::from(balance.0) * unplaced / whole;
    }

    let mut allocation = Allocation {
        slices: Vec::new(),
        covered,
        holdings: holdings.len(),
    };
    if basis <= 0 {
        return allocation;
    }

    let mut weights = [0i64; AssetClass::ALL.len()];
    // (class index, what the floor left owing) -- who is most owed the next
    // basis point.
    let mut fractions: Vec<(usize, i128)> = Vec::with_capacity(cents.len());
    let mut floors = 0i128;
    for (index, class_cents) in cents.iter().enumerate() {
        let numerator = whole * class_cents;
        let floor = numerator.div_euclid(basis);
        floors += floor;
        weights[index] = floor as i64;
        fractions.push((index, numerator.rem_euclid(basis)));
    }
    fractions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    let mut leftover = whole - floors;
    for (index, _) in &fractions {
        if leftover == 0 {
            break;
        }
        weights[*index] += 1;
        leftover -= 1;
    }

    allocation.slices = AssetClass::ALL
        .iter()
        .enumerate()
        .filter(|(index, class)| **class != AssetClass::Unclassified || weights[*index] != 0)
        .map(|(index, class)| Slice {
            class: *class,
            weight: BasisPoints(weights[index]),
        })
        .collect();
    allocation
}

fn index_of(class: AssetClass) -> usize {
    AssetClass::ALL
        .iter()
        .position(|c| *c == class)
        .expect("AssetClass::ALL names every variant")
}

/// A class the age rule has a target for.
///
/// Three rather than the six [`AssetClass`] carries, and `Bonds` is both bond
/// classes at once: the rule produces one bond number, and splitting it
/// between domestic and foreign would invent a precision it does not have.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TargetClass {
    Bonds,
    UsStock,
    IntlStock,
}

impl TargetClass {
    /// Every targeted class, in the order the summary lists them -- bonds
    /// first, being the share the age rule actually moves.
    pub const ALL: [TargetClass; 3] = [
        TargetClass::Bonds,
        TargetClass::UsStock,
        TargetClass::IntlStock,
    ];

    /// What the summary calls this class. `Bonds` is its own word; the two
    /// equity rows borrow [`AssetClass::label`], since they *are* that class
    /// and a second spelling of it would read as a second thing.
    pub fn label(self) -> &'static str {
        match self {
            TargetClass::Bonds => "Bonds",
            TargetClass::UsStock => AssetClass::UsStock.label(),
            TargetClass::IntlStock => AssetClass::IntlStock.label(),
        }
    }

    /// What the age rule targets here, or `None` for a bond share with no
    /// birth date behind it.
    pub fn target(self, targets: Targets) -> Option<BasisPoints> {
        match self {
            TargetClass::Bonds => targets.bonds,
            TargetClass::UsStock => Some(targets.us_stock),
            TargetClass::IntlStock => Some(targets.intl_stock),
        }
    }

    /// What the portfolio holds here.
    pub fn actual(self, slices: &[Slice]) -> BasisPoints {
        match self {
            TargetClass::Bonds => {
                weight(slices, AssetClass::UsBond) + weight(slices, AssetClass::IntlBond)
            }
            TargetClass::UsStock => weight(slices, AssetClass::UsStock),
            TargetClass::IntlStock => weight(slices, AssetClass::IntlStock),
        }
    }
}

/// One targeted class as the summary draws it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SummaryRow {
    pub class: TargetClass,
    /// `None` only for a bond share with no birth date on record, which both
    /// sinks draw as the `—` every other absence in the app draws.
    pub target: Option<BasisPoints>,
    pub actual: BasisPoints,
    /// `target - actual`, **signed**: over-weight in bonds is as much a thing
    /// to see as under-weight, and a column that could only report one
    /// direction would read as though the other never happened. `None`
    /// wherever `target` is, since there is nothing to be short of.
    pub delta: Option<BasisPoints>,
}

impl SummaryRow {
    pub fn new(class: TargetClass, slices: &[Slice], targets: Targets) -> SummaryRow {
        let target = class.target(targets);
        let actual = class.actual(slices);
        SummaryRow {
            class,
            target,
            actual,
            delta: target.map(|t| t - actual),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::fund;

    fn slices(pairs: &[(AssetClass, i64)]) -> Vec<Slice> {
        pairs
            .iter()
            .map(|(class, weight)| Slice {
                class: *class,
                weight: BasisPoints(*weight),
            })
            .collect()
    }

    /// A bond fund and an international stock fund, in balances that do not
    /// divide evenly into basis points -- which is the case the footing rule
    /// exists for.
    fn portfolio() -> (Vec<Slice>, Vec<Slice>) {
        (
            slices(&[
                (AssetClass::UsBond, 7_000),
                (AssetClass::IntlBond, 2_500),
                (AssetClass::Cash, 500),
            ]),
            slices(&[(AssetClass::IntlStock, 9_500), (AssetClass::Cash, 500)]),
        )
    }

    #[test]
    fn a_look_through_foots_to_a_whole_hundred_percent() {
        let (bond, intl) = portfolio();
        let held = [
            (Cents::from_dollars(5_000), Some(bond.as_slice())),
            (Cents::from_dollars(3_000), Some(intl.as_slice())),
        ];
        let allocation = apportion(&held);
        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0);
    }

    /// $5,000 of 70/25/5 bond fund and $3,000 of 95/5 international fund:
    /// $3,500 domestic bonds, $1,250 foreign bonds, $2,850 international
    /// stock and $400 cash, over $8,000. Two of those are exact halves of a
    /// basis point, and only one of them can round up.
    #[test]
    fn each_class_is_its_own_share_of_everything_the_mixes_place() {
        let (bond, intl) = portfolio();
        let held = [
            (Cents::from_dollars(5_000), Some(bond.as_slice())),
            (Cents::from_dollars(3_000), Some(intl.as_slice())),
        ];
        let allocation = apportion(&held);

        assert_eq!(
            weight(&allocation.slices, AssetClass::UsBond),
            BasisPoints(4_375)
        );
        assert_eq!(
            weight(&allocation.slices, AssetClass::Cash),
            BasisPoints(500)
        );
        // 35.625% and 15.625%: the whole basis point goes to the larger
        // remainder's tie-break, which is `AssetClass::ALL` order.
        assert_eq!(
            weight(&allocation.slices, AssetClass::IntlStock),
            BasisPoints(3_563)
        );
        assert_eq!(
            weight(&allocation.slices, AssetClass::IntlBond),
            BasisPoints(1_562)
        );
    }

    /// The rule Ruling-1 of this feature turns on: an unfetched holding is
    /// outside the denominator, so the classes still foot rather than each
    /// coming up short by the same unnamed fraction.
    #[test]
    fn a_holding_with_no_mix_is_outside_the_denominator_rather_than_shrinking_every_class() {
        let (bond, _) = portfolio();
        let covered = [(Cents::from_dollars(5_000), Some(bond.as_slice()))];
        let with_a_stranger = [
            (Cents::from_dollars(5_000), Some(bond.as_slice())),
            (Cents::from_dollars(90_000), None),
        ];

        assert_eq!(
            apportion(&covered).slices,
            apportion(&with_a_stranger).slices
        );
        assert_eq!(apportion(&with_a_stranger).covered, 1);
        assert_eq!(apportion(&with_a_stranger).holdings, 2);
    }

    /// A filing whose own slices come to 99% is a miss, and a miss has to
    /// surface as the labelled row it is rather than being renormalised away
    /// across the classes that were placed.
    #[test]
    fn what_a_mix_does_not_place_lands_in_unclassified_rather_than_renormalising() {
        let short = slices(&[(AssetClass::UsStock, 9_900)]);
        let held = [(Cents::from_dollars(1_000), Some(short.as_slice()))];
        let allocation = apportion(&held);

        assert_eq!(
            weight(&allocation.slices, AssetClass::UsStock),
            BasisPoints(9_900),
            "the placed share was inflated to cover the gap"
        );
        assert_eq!(
            weight(&allocation.slices, AssetClass::Unclassified),
            BasisPoints(100),
            "the gap went unreported"
        );
        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0);
    }

    /// Each slice's share of a balance is truncated to the cent, so a mix
    /// that foots perfectly can still leave a few cents over. That is dust
    /// rather than a miss, and an `Unclassified` row reading `0.01%` would
    /// report the classifier for the arithmetic's rounding.
    #[test]
    fn the_cents_a_perfect_mix_rounds_away_are_not_reported_as_unclassified() {
        let thirds = slices(&[
            (AssetClass::UsStock, 3_333),
            (AssetClass::IntlStock, 3_333),
            (AssetClass::UsBond, 3_334),
        ]);
        let held = [(Cents(10_001), Some(thirds.as_slice()))];
        let allocation = apportion(&held);

        assert!(
            !allocation
                .slices
                .iter()
                .any(|s| s.class == AssetClass::Unclassified),
            "rounding dust was drawn as a classifier miss: {:?}",
            allocation.slices
        );
        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0);
    }

    #[test]
    fn a_zero_unclassified_is_dropped_while_the_other_classes_keep_their_zeroes() {
        let (bond, _) = portfolio();
        let held = [(Cents::from_dollars(5_000), Some(bond.as_slice()))];
        let allocation = apportion(&held);

        assert!(
            !allocation
                .slices
                .iter()
                .any(|s| s.class == AssetClass::Unclassified)
        );
        assert!(
            allocation
                .slices
                .iter()
                .any(|s| s.class == AssetClass::UsStock && s.weight == BasisPoints::ZERO),
            "a class the targets are stated in lost its zero"
        );
    }

    #[test]
    fn an_unclassified_slice_the_classifier_could_not_place_is_kept() {
        let messy = slices(&[
            (AssetClass::UsStock, 9_000),
            (AssetClass::Unclassified, 1_000),
        ]);
        let held = [(Cents::from_dollars(1_000), Some(messy.as_slice()))];
        let allocation = apportion(&held);
        assert_eq!(
            weight(&allocation.slices, AssetClass::Unclassified),
            BasisPoints(1_000)
        );
    }

    /// Nothing fetched is not a portfolio that is all cash, and an empty
    /// composition is how the two are told apart.
    #[test]
    fn a_portfolio_with_no_mix_on_record_at_all_has_no_composition() {
        let held = [(Cents::from_dollars(5_000), None)];
        let allocation = apportion(&held);
        assert!(allocation.slices.is_empty());
        assert_eq!(allocation.covered, 0);
    }

    #[test]
    fn coverage_is_named_only_while_something_is_missing() {
        let (bond, _) = portfolio();
        let whole = [(Cents::from_dollars(5_000), Some(bond.as_slice()))];
        assert_eq!(apportion(&whole).coverage(), None);

        let partial = [
            (Cents::from_dollars(5_000), Some(bond.as_slice())),
            (Cents::from_dollars(1_000), None),
        ];
        assert_eq!(
            apportion(&partial).coverage().as_deref(),
            Some("1 of 2 holdings")
        );
    }

    #[test]
    fn the_bonds_row_is_both_bond_classes_and_its_delta_is_signed() {
        let (bond, intl) = portfolio();
        let held = [
            (Cents::from_dollars(5_000), Some(bond.as_slice())),
            (Cents::from_dollars(3_000), Some(intl.as_slice())),
        ];
        let allocation = apportion(&held);
        let targets = fund::targets(Some(48), BasisPoints(4_000));

        let row = SummaryRow::new(TargetClass::Bonds, &allocation.slices, targets);
        assert_eq!(row.actual, BasisPoints(4_375 + 1_562));
        assert_eq!(row.target, Some(BasisPoints(1_800)));
        assert_eq!(
            row.delta,
            Some(BasisPoints(1_800 - 5_937)),
            "an over-weight class must report which way it is out"
        );
    }

    #[test]
    fn a_bond_target_with_no_birth_date_behind_it_carries_no_delta_either() {
        let (bond, _) = portfolio();
        let held = [(Cents::from_dollars(5_000), Some(bond.as_slice()))];
        let allocation = apportion(&held);
        let targets = fund::targets(None, BasisPoints(4_000));

        let row = SummaryRow::new(TargetClass::Bonds, &allocation.slices, targets);
        assert_eq!(row.target, None);
        assert_eq!(row.delta, None);
        assert_eq!(
            row.actual,
            BasisPoints(9_500),
            "the actual is known whatever the target is"
        );
    }
}
