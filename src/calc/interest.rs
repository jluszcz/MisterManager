use crate::money::Cents;
use anyhow::{Result, ensure};

/// Split `total` across `balances` in proportion to each balance, in whole
/// dollars, summing to exactly `total`.
///
/// Those two requirements conflict, so this uses the largest-remainder
/// (Hamilton) method: every bucket gets its floor share, then the leftover
/// dollars go out one at a time to the buckets with the biggest fractional
/// remainders. Each share therefore lands within one dollar of its exact
/// value and, when every weight is non-negative, can never go negative --
/// callers that weight by a goal balance can pass a negative weight for an
/// overspent goal, and the method offers no such guarantee there.
///
/// Dumping the whole leftover on the single largest share -- the way
/// `Planning!D32` absorbs its plug -- is wrong here: when several buckets
/// round up at once the correction can exceed a share, and `$2.00` across
/// four equal buckets produces `$1, $1, $1, -$1`. The plug works in Planning
/// because one designated line absorbs it; here every share is a real
/// allocation to a real goal.
///
/// A negative `total` is an error, not a clamp: the largest-remainder method
/// below is derived for a non-negative total and would hand out shares that
/// do not sum to it. Interest postings come from the ledger, where a sign
/// flip means the caller picked up a withdrawal, and allocating that across
/// goals silently is worse than refusing. Any sub-dollar residue rides with
/// the largest fractional remainder.
pub fn pro_rata(total: Cents, balances: &[(i64, Cents)]) -> Result<Vec<(i64, Cents)>> {
    ensure!(
        total >= Cents::ZERO,
        "pro_rata expects a non-negative total, got {total}"
    );
    if balances.is_empty() {
        return Ok(Vec::new());
    }

    let basis: i128 = balances.iter().map(|(_, c)| c.0 as i128).sum();
    if basis <= 0 {
        // No proportions to work with.
        let mut shares: Vec<(i64, Cents)> =
            balances.iter().map(|(id, _)| (*id, Cents::ZERO)).collect();
        shares[0].1 = total;
        return Ok(shares);
    }

    // Whole dollars is the granularity the source spreadsheet allocates in.
    let whole_dollars = (total.0 / 100) as i128;
    let residue = Cents(total.0 % 100);

    let mut shares: Vec<(i64, Cents)> = Vec::with_capacity(balances.len());
    // (index, fractional remainder) — who is most owed the next whole dollar.
    let mut fractions: Vec<(usize, i128)> = Vec::with_capacity(balances.len());
    let mut floors_total: i128 = 0;

    for (index, (id, balance)) in balances.iter().enumerate() {
        let numerator = whole_dollars * balance.0 as i128;
        let floor = numerator.div_euclid(basis);
        floors_total += floor;
        shares.push((*id, Cents(floor as i64 * 100)));
        fractions.push((index, numerator.rem_euclid(basis)));
    }

    // Largest remainder first; ties break on input order so the result is
    // deterministic.
    fractions.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // Flooring loses at most one dollar per bucket, so this always fits.
    let mut leftover = whole_dollars - floors_total;
    for (index, _) in &fractions {
        if leftover == 0 {
            break;
        }
        shares[*index].1 += Cents(100);
        leftover -= 1;
    }

    if residue != Cents::ZERO {
        let top = fractions[0].0;
        shares[top].1 += residue;
    }

    Ok(shares)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::money::Cents;

    /// Every case below splits a valid, non-negative total, so unwrapping
    /// here keeps the assertions about the split itself. The error case has
    /// its own test.
    fn split(total: Cents, balances: &[(i64, Cents)]) -> Vec<(i64, Cents)> {
        pro_rata(total, balances).unwrap()
    }

    /// One container's interest posting, divided by balance. A third goal is
    /// excluded upstream by `interest_eligible`, which is why the denominator
    /// is these two alone.
    #[test]
    fn a_posting_divides_by_balance_across_the_eligible_goals() {
        let emergency = Cents(10_600_195); // 106,001.95
        let mom_and_dad = Cents(2_500_000); // 25,000.00
        let shares = split(
            Cents::from_dollars(2000),
            &[(1, emergency), (2, mom_and_dad)],
        );
        assert_eq!(
            shares,
            vec![
                (1, Cents::from_dollars(1618)),
                (2, Cents::from_dollars(382))
            ]
        );
    }

    #[test]
    fn always_sums_to_the_total() {
        // Three equal balances against $100 cannot divide evenly.
        let shares = split(
            Cents::from_dollars(100),
            &[(1, Cents(1000)), (2, Cents(1000)), (3, Cents(1000))],
        );
        let sum: Cents = shares.iter().map(|(_, c)| *c).sum();
        assert_eq!(sum, Cents::from_dollars(100));
        // 33 each leaves one dollar over; the input-order tiebreak gives it
        // to the first bucket.
        assert_eq!(
            shares,
            vec![
                (1, Cents::from_dollars(34)),
                (2, Cents::from_dollars(33)),
                (3, Cents::from_dollars(33))
            ]
        );
    }

    /// The leftover dollar must actually move, and it must land on the bucket
    /// with the largest fractional remainder.
    #[test]
    fn leftover_dollars_go_to_the_largest_remainders() {
        let shares = split(
            Cents::from_dollars(10),
            &[(1, Cents(100)), (2, Cents(9900))],
        );
        // Exact shares are 0.10 and 9.90: floors are 0 and 9, and the single
        // leftover dollar goes to the 0.90 fraction, not the 0.10 one.
        assert_eq!(shares, vec![(1, Cents::ZERO), (2, Cents::from_dollars(10))]);
    }

    /// The failure the largest-remainder method exists to prevent: dumping the
    /// whole leftover on one share yields [$1, $1, $1, -$1] here.
    #[test]
    fn never_assigns_a_negative_share() {
        let shares = split(
            Cents::from_dollars(2),
            &[(1, Cents(1)), (2, Cents(1)), (3, Cents(1)), (4, Cents(1))],
        );
        let sum: Cents = shares.iter().map(|(_, c)| *c).sum();
        assert_eq!(sum, Cents::from_dollars(2));
        assert!(
            shares.iter().all(|(_, c)| *c >= Cents::ZERO),
            "negative share in {shares:?}"
        );
    }

    /// No share may drift more than a dollar from its exact proportion.
    #[test]
    fn every_share_stays_within_a_dollar_of_exact() {
        let balances = [(1, Cents(333)), (2, Cents(333)), (3, Cents(334))];
        let total = Cents::from_dollars(7);
        let shares = split(total, &balances);
        let basis: i128 = balances.iter().map(|(_, c)| c.0 as i128).sum();
        for ((_, share), (_, balance)) in shares.iter().zip(balances.iter()) {
            let exact = total.0 as i128 * balance.0 as i128 / basis;
            assert!(
                (share.0 as i128 - exact).abs() < 100,
                "share {share} is more than a dollar from exact {exact}"
            );
        }
    }

    /// Interest rows are not whole dollars, and the residue of dividing one
    /// has to land somewhere rather than evaporate.
    #[test]
    fn sub_dollar_residue_still_sums_exactly() {
        let total = Cents(120_048);
        let shares = split(total, &[(1, Cents(10_600_195)), (2, Cents(2_500_000))]);
        let sum: Cents = shares.iter().map(|(_, c)| *c).sum();
        assert_eq!(sum, total);
    }

    #[test]
    fn empty_input_yields_no_shares() {
        assert_eq!(split(Cents::from_dollars(100), &[]), vec![]);
    }

    #[test]
    fn zero_balances_give_everything_to_the_first_bucket() {
        let shares = split(
            Cents::from_dollars(60),
            &[(1, Cents::ZERO), (2, Cents::ZERO)],
        );
        // Assert the identity, not just the sum: a test that only checks the
        // total would pass if the money landed on bucket 2.
        assert_eq!(shares, vec![(1, Cents::from_dollars(60)), (2, Cents::ZERO)]);
    }

    /// A negative total must be refused in every build, release included. A
    /// debug-only assertion would let a release build carry on and hand back
    /// shares that do not sum to the total.
    #[test]
    fn a_negative_total_is_refused() {
        let err = pro_rata(Cents::from_dollars(-5), &[(1, Cents(100))]).unwrap_err();
        assert!(err.to_string().contains("non-negative"), "{err}");
    }
}
