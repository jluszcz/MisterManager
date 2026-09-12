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
//! that mix failed to place lands in `Unclassified`, which is
//! [`apportion`]'s to argue.

use crate::calc::fund::Targets;
use crate::db::account::TaxTreatment;
use crate::db::fund_mix::{AssetClass, Slice};
use crate::money::Cents;
use crate::rate::BasisPoints;

/// The four groups the summary draws, in the order it draws them: the share
/// the age rule actually moves, the two equities it splits the rest between,
/// and everything else.
///
/// **One list for the rows and the bar both.** They were two -- three targeted
/// classes in a table and four `AssetClass`es in a bar -- and a reader had to
/// hold that the bar split a bond number the row above it could not. A class
/// owns its label, what it is made of, and whether the rule has anything to
/// say about it, so the table and the bar are two spellings of one sequence
/// rather than two answers about one portfolio.
///
/// It is deliberately coarser than [`AssetClass`], which keeps all six:
/// `fund_mix` stores the bond split and `mix::classify` still finds it, so
/// nothing is lost on the way in. What collapses is the drawing.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Class {
    Bonds,
    UsStock,
    IntlStock,
    Other,
}

impl Class {
    /// Every class, in the order the summary lists them -- bonds first, being
    /// the share the age rule actually moves, and `Other` last, being what
    /// the rule says nothing about.
    pub const ALL: [Class; 4] = [Class::Bonds, Class::UsStock, Class::IntlStock, Class::Other];

    /// What the summary calls this class.
    ///
    /// `Bonds` and `Other` are their own words; the two equity classes borrow
    /// [`AssetClass::label`], since they *are* that class and a second
    /// spelling of it would read as a second thing.
    pub fn label(self) -> &'static str {
        match self {
            Class::Bonds => "Bonds",
            Class::UsStock => AssetClass::UsStock.label(),
            Class::IntlStock => AssetClass::IntlStock.label(),
            Class::Other => "Other",
        }
    }

    /// The [`AssetClass`]es this class is made of.
    ///
    /// `Bonds` is both bond classes at once because the age rule produces one
    /// bond number, and splitting it between domestic and foreign would
    /// invent a precision it does not have. `Other` is cash and the
    /// classifier's residual together: the rule has nothing to say about
    /// either, and two rows of what it is not asking about is two rows
    /// nobody reads.
    pub fn classes(self) -> &'static [AssetClass] {
        match self {
            Class::Bonds => &[AssetClass::UsBond, AssetClass::IntlBond],
            Class::UsStock => &[AssetClass::UsStock],
            Class::IntlStock => &[AssetClass::IntlStock],
            Class::Other => &[AssetClass::Cash, AssetClass::Unclassified],
        }
    }

    /// This class's own place in [`Class::ALL`].
    ///
    /// `palette::CLASSES` is keyed by it, for the reason `AssetClass::index`
    /// exists: the colors are reached by position, and a second copy of this
    /// mapping is a reorder away from repainting every segment.
    pub fn index(self) -> usize {
        Class::ALL
            .iter()
            .position(|class| *class == self)
            .expect("Class::ALL names every variant")
    }

    /// What the age rule targets here, or `None` where it has nothing to say
    /// -- `Other` always, and `Bonds` with no birth date behind it.
    pub fn target(self, targets: Targets) -> Option<BasisPoints> {
        match self {
            Class::Bonds => targets.bonds,
            Class::UsStock => Some(targets.us_stock),
            Class::IntlStock => Some(targets.intl_stock),
            Class::Other => None,
        }
    }

    /// What the portfolio holds here.
    pub fn actual(self, slices: &[Slice]) -> BasisPoints {
        self.classes()
            .iter()
            .fold(BasisPoints::ZERO, |sum, class| sum + weight(slices, *class))
    }
}

/// The portfolio's composition, and how much of it the composition is of.
///
/// `slices` foots to [`BasisPoints::ONE`] whenever there is anything to
/// apportion, and is empty when there is not -- a portfolio nobody has
/// fetched a mix for has no composition, which is a different statement from
/// one that is all cash.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Allocation {
    /// One entry per [`AssetClass`], in `AssetClass::ALL` order.
    ///
    /// Every class keeps its zero, `Unclassified` included: they are the
    /// vocabulary the targets and the bar are stated in, so a zero is an
    /// answer rather than a gap. The residual used to be dropped when it was
    /// nothing, a defect report reading "none" every time being one nobody
    /// finishes reading -- but [`Class::Other`] draws it beside cash now, so
    /// the row exists whatever the residual is and there is nothing left for
    /// the omission to spare a reader.
    pub slices: Vec<Slice>,
    /// [`slices`](Allocation::slices) again, split by the tax treatment of
    /// the account each balance sits in: one entry per [`TaxTreatment::ALL`]
    /// member, in that order.
    ///
    /// **Each is a share of the whole covered balance, not of its own
    /// treatment**, so a class's three columns sum to its `actual` and a
    /// treatment's four classes sum to what that treatment holds. Read either
    /// way round, the figures are about one portfolio.
    ///
    /// A holding in an account with no treatment on record is in `slices` and
    /// in none of these, so the three columns visibly sum short rather than
    /// landing somewhere they were never said to be. The schema's paired
    /// `CHECK` makes that unreachable through the app -- `tax_treatment` is
    /// present exactly when the kind is `investment`, and a holding has
    /// nowhere else to live -- which is why it is a shortfall to notice
    /// rather than an error to refuse on.
    pub treatments: [Vec<Slice>; TaxTreatment::ALL.len()],
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

    /// What one class holds inside one tax treatment.
    ///
    /// [`Class::actual`] read over that treatment's own slices rather than
    /// over the whole portfolio's, which is the only difference between this
    /// and the `Actual` column beside it.
    pub fn class_in(&self, class: Class, treatment: TaxTreatment) -> BasisPoints {
        class.actual(&self.treatments[treatment.index()])
    }

    /// What one tax treatment holds, across every class.
    pub fn treatment_total(&self, treatment: TaxTreatment) -> BasisPoints {
        Class::ALL.iter().fold(BasisPoints::ZERO, |sum, class| {
            sum + self.class_in(*class, treatment)
        })
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

/// One holding, as the look-through reads it: what it is worth, how the
/// account holding it is taxed, and what its fund is made of.
///
/// A struct rather than a tuple because the second and third fields are both
/// optional and neither is the other's kind of absence -- `None` for a mix is
/// a fund nobody has fetched, and `None` for a treatment is a database the
/// schema says cannot exist.
#[derive(Copy, Clone, Debug)]
pub struct Held<'a> {
    pub balance: Cents,
    pub treatment: Option<TaxTreatment>,
    pub mix: Option<&'a [Slice]>,
}

/// Where a holding's balance accumulates: one column per [`TaxTreatment::ALL`]
/// member, and a last for a holding whose account states none.
///
/// The unstated column is carried through the apportioning and drawn by
/// nobody. It has to be carried, or the classes would foot to less than the
/// portfolio and every share would be quietly inflated; it is not drawn,
/// because there is no honest label for it -- see
/// [`Allocation::treatments`].
const COLUMNS: usize = TaxTreatment::ALL.len() + 1;

/// Which column a holding's treatment accumulates in.
fn column(treatment: Option<TaxTreatment>) -> usize {
    treatment.map_or(TaxTreatment::ALL.len(), TaxTreatment::index)
}

/// Each holding's balance apportioned by its fund's composition, summed by
/// class and by the tax treatment it sits in, as basis points of the covered
/// balance.
///
/// `None` for a holding's mix is a fund nobody has fetched: it counts toward
/// [`Allocation::holdings`] and toward nothing else.
///
/// **What a mix does not place lands in `Unclassified`.** `mix::classify` is
/// what guards a composition's own footing, and it guards it to within its
/// own rounding -- so a mix this app fetched arrives whole, and the gap that
/// filing left is already an `Unclassified` slice of its own by the time it
/// reaches here. What is *not* guarded is `fund_mix` itself: it is an
/// ordinary table with no constraint on what its rows sum to, and a row put
/// there by anything but a refresh -- a hand edit, a restored database, a
/// second writer -- can come to 99% with nothing to stop it. The two other
/// answers to that both lose the fact. Dividing by what was placed instead of
/// by the balance renormalises the gap away across the classes that *were*
/// placed, which says nothing; leaving it out would foot to 99% in a table
/// whose one unreadable state is a total that does not foot. `Unclassified`
/// is the class that exists for exactly this, drawn inside `Other`, so
/// routing the gap there foots *and* keeps it in a labelled bucket. A mix
/// that over-foots surfaces the same way, as a negative.
///
/// Truncating each share and dividing the leftover by largest remainder is
/// [`crate::calc::interest::pro_rata`]'s method and is here for its reason:
/// the shares have to foot exactly, and dumping the whole leftover on the
/// largest of them can exceed a share when several round up at once. Ties
/// break on `AssetClass::ALL` order then column order, so one portfolio has
/// one summary.
///
/// **The rounding runs once, over the whole class-by-treatment grid**, rather
/// than once per class and again per treatment. Two passes would each foot to
/// a hundred on their own and still disagree with each other by the point one
/// of them rounded differently, which is the one thing a table whose rows and
/// columns are both read has to rule out. So the grid foots, and both
/// summaries are sums over it.
pub fn apportion(holdings: &[Held<'_>]) -> Allocation {
    let covered = holdings.iter().filter(|held| held.mix.is_some()).count();
    let whole = i128::from(BasisPoints::ONE.0);
    // Cents times basis points, undivided. A share resolved to whole cents
    // here would leave the classes summing to less than the balance they came
    // from, by up to a cent per cell per holding -- and the largest remainder
    // below has exactly one basis point per cell to give, so a gap that grows
    // with the holdings is one it cannot close. Carried in the product, every
    // holding contributes its balance exactly, which is what leaves the
    // leftover inside what the method can divide. The gap a mix itself left
    // rides in the same unit, being the same arithmetic about the same
    // balance.
    let mut scaled = [[0i128; COLUMNS]; AssetClass::ALL.len()];
    let mut basis = 0i128;
    for held in holdings {
        let Some(mix) = held.mix else { continue };
        let column = column(held.treatment);
        basis += i128::from(held.balance.0);
        for slice in mix {
            scaled[slice.class.index()][column] +=
                i128::from(held.balance.0) * i128::from(slice.weight.0);
        }
        let unplaced = whole - mix.iter().map(|s| i128::from(s.weight.0)).sum::<i128>();
        scaled[AssetClass::Unclassified.index()][column] += i128::from(held.balance.0) * unplaced;
    }

    let mut allocation = Allocation {
        covered,
        holdings: holdings.len(),
        ..Allocation::default()
    };
    if basis <= 0 {
        return allocation;
    }

    let mut weights = [[0i64; COLUMNS]; AssetClass::ALL.len()];
    // (class, column, what the floor left owing) -- who is most owed the next
    // basis point.
    let mut fractions: Vec<(usize, usize, i128)> = Vec::with_capacity(scaled.len() * COLUMNS);
    let mut floors = 0i128;
    for (class, columns) in scaled.iter().enumerate() {
        for (column, cell) in columns.iter().enumerate() {
            let floor = cell.div_euclid(basis);
            floors += floor;
            weights[class][column] = floor as i64;
            fractions.push((class, column, cell.rem_euclid(basis)));
        }
    }
    fractions.sort_by(|a, b| b.2.cmp(&a.2).then((a.0, a.1).cmp(&(b.0, b.1))));

    let mut leftover = whole - floors;
    for (class, column, _) in &fractions {
        if leftover == 0 {
            break;
        }
        weights[*class][*column] += 1;
        leftover -= 1;
    }

    allocation.slices = AssetClass::ALL
        .iter()
        .enumerate()
        .map(|(class, name)| Slice {
            class: *name,
            weight: BasisPoints(weights[class].iter().sum()),
        })
        .collect();
    allocation.treatments = std::array::from_fn(|column| {
        AssetClass::ALL
            .iter()
            .enumerate()
            .map(|(class, name)| Slice {
                class: *name,
                weight: BasisPoints(weights[class][column]),
            })
            .collect()
    });
    allocation
}

/// One class as the summary draws it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SummaryRow {
    pub class: Class,
    /// `None` where the age rule has nothing to say: `Other` always, and a
    /// bond share with no birth date on record. Both sinks draw it as the
    /// `—` every other absence in the app draws.
    pub target: Option<BasisPoints>,
    pub actual: BasisPoints,
    /// `actual - target`, **signed**: over-weight in bonds is as much a thing
    /// to see as under-weight, and a column that could only report one
    /// direction would read as though the other never happened. `None`
    /// wherever `target` is, since there is nothing to be short of.
    ///
    /// **Actual first, so the sign reads as a direction on the portfolio.**
    /// The two columns beside it are the rule's ask and what is held, and a
    /// reader arriving at the third has just read them left to right: a
    /// class held past its target is *more*, a positive number, and one held
    /// under it is the shortfall the negative colour marks. Subtracted the
    /// other way the figure is a correction -- what would have to be moved --
    /// and it paints red exactly the rows a reader is already over on.
    pub delta: Option<BasisPoints>,
}

impl SummaryRow {
    pub fn new(class: Class, slices: &[Slice], targets: Targets) -> SummaryRow {
        let target = class.target(targets);
        let actual = class.actual(slices);
        SummaryRow {
            class,
            target,
            actual,
            delta: target.map(|t| actual - t),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calc::fund;

    /// One holding, in the treatment every fixture here uses unless it is
    /// making a point about the split.
    fn held(dollars: i64, mix: Option<&[Slice]>) -> Held<'_> {
        Held {
            balance: Cents::from_dollars(dollars),
            treatment: Some(TaxTreatment::Taxable),
            mix,
        }
    }

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

    /// The footing does not depend on the balance being large enough for a
    /// class's share to land on a whole cent. A dollar split three ways
    /// resolves to whole cents nowhere, and a summary short by the rounding
    /// would draw an `Unclassified` row where nothing went unplaced.
    #[test]
    fn a_look_through_of_a_balance_below_a_cent_a_class_still_foots() {
        let mix = slices(&[
            (AssetClass::UsStock, 3_333),
            (AssetClass::IntlStock, 3_333),
            (AssetClass::UsBond, 3_334),
        ]);
        let held = [Held {
            balance: Cents(100),
            treatment: Some(TaxTreatment::Taxable),
            mix: Some(mix.as_slice()),
        }];
        let allocation = apportion(&held);
        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0);
        assert_eq!(
            weight(&allocation.slices, AssetClass::Unclassified),
            BasisPoints::ZERO,
            "a mix that places everything leaves nothing unplaced"
        );
    }

    #[test]
    fn a_look_through_foots_to_a_whole_hundred_percent() {
        let (bond, intl) = portfolio();
        let held = [
            held(5_000, Some(bond.as_slice())),
            held(3_000, Some(intl.as_slice())),
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
    fn each_class_is_its_own_share_of_the_covered_balance() {
        let (bond, intl) = portfolio();
        let held = [
            held(5_000, Some(bond.as_slice())),
            held(3_000, Some(intl.as_slice())),
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
        let covered = [held(5_000, Some(bond.as_slice()))];
        let with_a_stranger = [held(5_000, Some(bond.as_slice())), held(90_000, None)];

        assert_eq!(
            apportion(&covered).slices,
            apportion(&with_a_stranger).slices
        );
        assert_eq!(apportion(&with_a_stranger).covered, 1);
        assert_eq!(apportion(&with_a_stranger).holdings, 2);
    }

    /// `mix::classify` foots what it writes, so a `fund_mix` row coming to
    /// 99% is one nothing in this crate wrote -- a hand edit, a restored
    /// database. It is still a miss, and a miss has to surface as the
    /// labelled row it is rather than being renormalised away across the
    /// classes that were placed.
    #[test]
    fn what_a_mix_does_not_place_lands_in_unclassified_rather_than_renormalising() {
        let short = slices(&[(AssetClass::UsStock, 9_900)]);
        let held = [held(1_000, Some(short.as_slice()))];
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

    /// A mix that foots perfectly leaves nothing unplaced at any balance,
    /// so the residual is a flat zero rather than the basis point the
    /// arithmetic's own rounding could otherwise leave in it.
    #[test]
    fn the_rounding_of_a_perfect_mix_is_not_reported_as_unclassified() {
        let thirds = slices(&[
            (AssetClass::UsStock, 3_333),
            (AssetClass::IntlStock, 3_333),
            (AssetClass::UsBond, 3_334),
        ]);
        let held = [Held {
            balance: Cents(10_001),
            treatment: Some(TaxTreatment::Taxable),
            mix: Some(thirds.as_slice()),
        }];
        let allocation = apportion(&held);

        assert_eq!(
            weight(&allocation.slices, AssetClass::Unclassified),
            BasisPoints::ZERO,
            "rounding dust was drawn as a classifier miss: {:?}",
            allocation.slices
        );
        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0);
    }

    /// Every class keeps its zero, the residual included: `Class::Other`
    /// draws it beside the cash whatever it holds, so there is no row for an
    /// omission to spare a reader and a missing slice would only make the
    /// vocabulary uneven.
    #[test]
    fn every_class_keeps_its_zero_including_the_residual() {
        let (bond, _) = portfolio();
        let held = [held(5_000, Some(bond.as_slice()))];
        let allocation = apportion(&held);

        assert!(
            allocation
                .slices
                .iter()
                .any(|s| s.class == AssetClass::Unclassified && s.weight == BasisPoints::ZERO),
            "the residual lost its zero"
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
        let held = [held(1_000, Some(messy.as_slice()))];
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
        let held = [held(5_000, None)];
        let allocation = apportion(&held);
        assert!(allocation.slices.is_empty());
        assert_eq!(allocation.covered, 0);
    }

    /// The grid is rounded once, so it foots in both directions: every
    /// class's three columns come to its own share, and every treatment's
    /// classes come to what that treatment holds. Two passes would each foot
    /// on their own and disagree with each other.
    #[test]
    fn the_tax_columns_foot_across_to_a_class_and_down_to_a_treatment() {
        let (bond, intl) = portfolio();
        let held = [
            Held {
                balance: Cents::from_dollars(5_000),
                treatment: Some(TaxTreatment::Taxable),
                mix: Some(bond.as_slice()),
            },
            Held {
                balance: Cents::from_dollars(3_000),
                treatment: Some(TaxTreatment::TaxDeferred),
                mix: Some(intl.as_slice()),
            },
        ];
        let allocation = apportion(&held);

        for class in Class::ALL {
            let across: i64 = TaxTreatment::ALL
                .iter()
                .map(|t| allocation.class_in(class, *t).0)
                .sum();
            assert_eq!(
                across,
                class.actual(&allocation.slices).0,
                "{class:?} does not foot across its treatments"
            );
        }
        let down: i64 = TaxTreatment::ALL
            .iter()
            .map(|t| allocation.treatment_total(*t).0)
            .sum();
        assert_eq!(down, BasisPoints::ONE.0, "the treatments do not foot");

        // The bond fund is the taxable one and the international fund the
        // deferred one, so each treatment holds exactly its own fund.
        assert_eq!(
            allocation.treatment_total(TaxTreatment::TaxFree),
            BasisPoints::ZERO
        );
        assert_eq!(
            allocation.class_in(Class::IntlStock, TaxTreatment::Taxable),
            BasisPoints::ZERO,
            "the international fund is not held in the taxable account"
        );
    }

    /// A holding whose account states no treatment is in the portfolio and
    /// in none of the columns, so the three visibly sum short rather than
    /// landing somewhere the database never said.
    #[test]
    fn a_holding_with_no_treatment_counts_in_the_class_and_in_no_column() {
        let (bond, _) = portfolio();
        let held = [
            Held {
                balance: Cents::from_dollars(5_000),
                treatment: Some(TaxTreatment::Taxable),
                mix: Some(bond.as_slice()),
            },
            Held {
                balance: Cents::from_dollars(5_000),
                treatment: None,
                mix: Some(bond.as_slice()),
            },
        ];
        let allocation = apportion(&held);

        let total: i64 = allocation.slices.iter().map(|s| s.weight.0).sum();
        assert_eq!(total, BasisPoints::ONE.0, "the classes have to foot");
        let columns: i64 = TaxTreatment::ALL
            .iter()
            .map(|t| allocation.treatment_total(*t).0)
            .sum();
        assert_eq!(
            columns,
            BasisPoints::ONE.0 / 2,
            "the untreated half was quietly given a treatment"
        );
    }

    #[test]
    fn coverage_is_named_only_while_something_is_missing() {
        let (bond, _) = portfolio();
        let whole = [held(5_000, Some(bond.as_slice()))];
        assert_eq!(apportion(&whole).coverage(), None);

        let partial = [held(5_000, Some(bond.as_slice())), held(1_000, None)];
        assert_eq!(
            apportion(&partial).coverage().as_deref(),
            Some("1 of 2 holdings")
        );
    }

    #[test]
    fn the_bonds_row_is_both_bond_classes_and_its_delta_is_signed() {
        let (bond, intl) = portfolio();
        let held = [
            held(5_000, Some(bond.as_slice())),
            held(3_000, Some(intl.as_slice())),
        ];
        let allocation = apportion(&held);
        let targets = fund::targets(Some(48), BasisPoints(4_000));

        let row = SummaryRow::new(Class::Bonds, &allocation.slices, targets);
        assert_eq!(row.actual, BasisPoints(4_375 + 1_562));
        assert_eq!(row.target, Some(BasisPoints(1_800)));
        assert_eq!(
            row.delta,
            Some(BasisPoints(5_937 - 1_800)),
            "an over-weight class must report which way it is out, and it is over"
        );
    }

    #[test]
    fn a_bond_target_with_no_birth_date_behind_it_carries_no_delta_either() {
        let (bond, _) = portfolio();
        let held = [held(5_000, Some(bond.as_slice()))];
        let allocation = apportion(&held);
        let targets = fund::targets(None, BasisPoints(4_000));

        let row = SummaryRow::new(Class::Bonds, &allocation.slices, targets);
        assert_eq!(row.target, None);
        assert_eq!(row.delta, None);
        assert_eq!(
            row.actual,
            BasisPoints(9_500),
            "the actual is known whatever the target is"
        );
    }
}
