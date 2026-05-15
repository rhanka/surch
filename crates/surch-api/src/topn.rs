//! Scalar top-N collector backed by a small sorted array.
//!
//! `TopN<T>` keeps at most `capacity` items in sorted order according to
//! a user-provided comparator. The comparator follows the standard
//! `cmp(a, b)` convention: items that compare as `Less` come *first* in
//! the resulting sorted vector — i.e. the "best" items.
//!
//! Hot-path semantics:
//!
//! - Below capacity: binary-search + insert (O(K) shift, but K is small).
//! - At capacity: compare against the current worst (the last slot, which
//!   is `O(1)`). Only beat that worst do we pay for a binary-search +
//!   insert + truncate.
//!
//! This avoids the `select_nth_unstable_by + sort_by` pair that pays
//! O(N) partition + O(K log K) tail-sort at the end. Tantivy 0.22
//! reported ~+15 % on top-K finalization after migrating to the same
//! shape against its previous `BinaryHeap`-based collector.

use std::cmp::Ordering;

/// Bounded sorted collector. Best items (those that compare as `Less`)
/// stay at the front; the worst kept item is at `data.last()`.
pub(crate) struct TopN<T, F>
where
    F: Fn(&T, &T) -> Ordering,
{
    capacity: usize,
    data: Vec<T>,
    cmp: F,
}

impl<T, F> TopN<T, F>
where
    F: Fn(&T, &T) -> Ordering,
{
    /// Build an empty collector that will retain at most `capacity`
    /// items. `capacity == 0` is allowed and accepts nothing.
    pub(crate) fn new(capacity: usize, cmp: F) -> Self {
        Self {
            capacity,
            data: Vec::with_capacity(capacity),
            cmp,
        }
    }

    /// Offer a candidate. Hot path: if the buffer is full, an `O(1)`
    /// comparison against the current worst decides whether we pay the
    /// insertion cost.
    pub(crate) fn push(&mut self, item: T) {
        if self.capacity == 0 {
            return;
        }

        if self.data.len() < self.capacity {
            let pos = self
                .data
                .binary_search_by(|probe| (self.cmp)(probe, &item))
                .unwrap_or_else(|e| e);
            self.data.insert(pos, item);
            return;
        }

        // Buffer full: only insert if item is strictly better than the
        // current worst (the last slot). Equal-ranked items lose to the
        // incumbent — the comparator carries the tie-break.
        let worst = self.data.last().expect("non-empty when at capacity");
        if (self.cmp)(&item, worst) != Ordering::Less {
            return;
        }

        let pos = self
            .data
            .binary_search_by(|probe| (self.cmp)(probe, &item))
            .unwrap_or_else(|e| e);
        self.data.insert(pos, item);
        self.data.truncate(self.capacity);
    }

    /// Consume the collector and return the retained items in sorted
    /// order (best first).
    pub(crate) fn into_sorted_vec(self) -> Vec<T> {
        self.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn desc_then_id(a: &(f64, u32), b: &(f64, u32)) -> Ordering {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    }

    #[test]
    fn capacity_zero_accepts_nothing() {
        let mut top = TopN::new(0, desc_then_id);
        top.push((1.0, 0));
        top.push((2.0, 1));
        assert!(top.into_sorted_vec().is_empty());
    }

    #[test]
    fn capacity_one_keeps_best() {
        let mut top = TopN::new(1, desc_then_id);
        top.push((1.0, 7));
        top.push((3.0, 4));
        top.push((2.0, 9));
        assert_eq!(top.into_sorted_vec(), vec![(3.0, 4)]);
    }

    #[test]
    fn ten_items_capacity_five() {
        let items = [
            (0.1, 0),
            (0.9, 1),
            (0.5, 2),
            (0.7, 3),
            (0.2, 4),
            (0.6, 5),
            (0.4, 6),
            (0.8, 7),
            (0.3, 8),
            (1.0, 9),
        ];
        let mut top = TopN::new(5, desc_then_id);
        for it in items {
            top.push(it);
        }
        let got = top.into_sorted_vec();
        assert_eq!(got, vec![(1.0, 9), (0.9, 1), (0.8, 7), (0.7, 3), (0.6, 5)]);
    }

    #[test]
    fn random_order_matches_reference_sort() {
        // Construct a reference top-K by sorting the full set; the
        // collector must reach the same answer regardless of arrival
        // order.
        let raw: Vec<(f64, u32)> = (0..50)
            .map(|i| {
                let score = ((i * 37) % 100) as f64 / 100.0;
                (score, i as u32)
            })
            .collect();

        // shuffled-ish: rotate by a prime offset
        let mut shuffled = raw.clone();
        shuffled.rotate_left(17);

        let mut reference = raw.clone();
        reference.sort_by(desc_then_id);
        reference.truncate(8);

        let mut top = TopN::new(8, desc_then_id);
        for it in shuffled {
            top.push(it);
        }
        assert_eq!(top.into_sorted_vec(), reference);
    }

    #[test]
    fn ties_break_on_secondary_key() {
        // All same score: the comparator tie-breaks on ascending doc_id,
        // so doc_id 1..=5 should win over 6..=10.
        let mut top = TopN::new(5, desc_then_id);
        for id in (1..=10).rev() {
            top.push((0.5, id));
        }
        let got = top.into_sorted_vec();
        assert_eq!(got, vec![(0.5, 1), (0.5, 2), (0.5, 3), (0.5, 4), (0.5, 5)]);
    }

    #[test]
    fn equal_items_first_in_wins() {
        // When a candidate compares Equal to the current worst, the
        // incumbent must keep its slot — `push` bails on `!= Less`.
        // Build a full buffer with two distinct items, then offer
        // duplicates of each: nothing should change.
        let mut top = TopN::new(2, |a: &u32, b: &u32| a.cmp(b));
        top.push(5);
        top.push(7);
        let before = top.into_sorted_vec();
        assert_eq!(before, vec![5, 7]);

        let mut top = TopN::new(2, |a: &u32, b: &u32| a.cmp(b));
        top.push(5);
        top.push(7);
        top.push(7); // tied with worst incumbent — rejected
        top.push(5); // strictly best incumbent already present — but
                     // 5 < 7, so this DOES displace 7 since 5 is
                     // strictly better than the current worst. The
                     // resulting buffer is [5, 5].
        assert_eq!(top.into_sorted_vec(), vec![5, 5]);

        // Now the actual tie-vs-worst invariant: duplicates of the
        // current worst must never displace it.
        let mut top = TopN::new(2, |a: &u32, b: &u32| a.cmp(b));
        top.push(5);
        top.push(7);
        for _ in 0..5 {
            top.push(7);
        }
        assert_eq!(top.into_sorted_vec(), vec![5, 7]);
    }
}
