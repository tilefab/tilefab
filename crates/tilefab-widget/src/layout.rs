//! Layout of a [`LineWidget`](crate::LineWidget): which components are kept and how wide each
//! flexible one is, for an available width `W`. A pure function, with no buffer and no
//! allocation.
//!
//! Algorithm, run at every draw, on the components *kept* (all of them at first):
//!
//! 1. `A = W −` the widths of the fixed components;
//! 2. if `A` covers the nominal widths of the flexible components, each one gets its nominal
//!    width and the receiver (if kept) the rest;
//! 3. otherwise `L` and `I` shrink together, in proportion to their nominal widths, down to
//!    2 columns each (the `N` keep their nominal width);
//! 4. otherwise, with `L` and `I` at 2, the `N` shrink together the same way, down to 2;
//! 5. otherwise the last kept component is dropped, and the layout starts again.
//!
//! Proportional sharing of a budget `B` among members of nominal widths `wᵢ` (total `T`):
//! each gets `floor(wᵢ × B / T)`, at least its minimum; members held at their minimum leave
//! the sharing and the rest of the budget is shared again among the others; the columns left
//! by rounding go one by one to the remaining members in composition order.
//!
//! The sharing needs no list of the members held: the ratio `B / T` only goes down from one
//! pass to the next, so a member held once stays held, and the final set of held members is
//! the set of members whose share at the *final* ratio is below their minimum. A [`Share`]
//! therefore keeps only the final budget and total, and the width of each member is computed
//! again when it is drawn.

use crate::widget::MIN_FLEXIBLE_WIDTH;

/// One component, as the layout sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Item {
    /// A component that never changes width.
    Fixed(usize),
    /// `L` or `I`, with its nominal width (at least 2).
    Shrink(usize),
    /// `N`, with the width of its current entry.
    Name(usize),
}

/// Smallest width of a flexible member of nominal width `nominal`: 2, or less for a narrower
/// name (a name 1 column wide never grows).
fn minimum(nominal: usize) -> usize {
    nominal.min(MIN_FLEXIBLE_WIDTH)
}

/// How the members of a group (`L` and `I`, or the `N`) get their width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Share {
    /// Each member has its nominal width.
    Nominal,
    /// Each member has its minimum width.
    Minimum,
    /// Proportional sharing: a member whose share `floor(w × budget / total)` is below its
    /// minimum is held at the minimum; the others get their share, and the first `leftover`
    /// of them in composition order one column more.
    Shared {
        /// Budget left to the members not held.
        budget: usize,
        /// Sum of the nominal widths of the members not held (never 0).
        total: usize,
        /// Columns left by rounding.
        leftover: usize,
    },
}

/// `floor(nominal × budget / total)`, without overflow.
fn proportion(nominal: usize, budget: usize, total: usize) -> usize {
    let share = (nominal as u128) * (budget as u128) / (total as u128);
    usize::try_from(share).unwrap_or(usize::MAX)
}

impl Share {
    /// Share `budget` among the members whose nominal widths `members` gives (called again
    /// at each pass; at least one member, and `budget` at least the sum of their minimums and
    /// below the sum of their nominal widths).
    fn proportional<I: Iterator<Item = usize>>(budget: usize, members: impl Fn() -> I) -> Self {
        let nominal_total: usize = members().sum();
        let (mut rest, mut total) = (budget, nominal_total);
        loop {
            // The members held at the current ratio.
            let (mut held_minimum, mut held_nominal) = (0, 0);
            for nominal in members() {
                if proportion(nominal, rest, total) < minimum(nominal) {
                    held_minimum += minimum(nominal);
                    held_nominal += nominal;
                }
            }
            let next = (budget - held_minimum, nominal_total - held_nominal);
            if next == (rest, total) {
                break;
            }
            (rest, total) = next;
            if total == 0 {
                // Every member is held.
                return Self::Minimum;
            }
        }
        let given: usize = members()
            .map(|nominal| (nominal, proportion(nominal, rest, total)))
            .filter(|&(nominal, share)| share >= minimum(nominal))
            .map(|(_, share)| share)
            .sum();
        Self::Shared {
            budget: rest,
            total,
            leftover: rest - given,
        }
    }

    /// Width of a member of nominal width `nominal`; `active` counts the members not held
    /// that come before it, and is incremented when this one is not held either.
    fn width(self, nominal: usize, active: &mut usize) -> usize {
        match self {
            Self::Nominal => nominal,
            Self::Minimum => minimum(nominal),
            Self::Shared {
                budget,
                total,
                leftover,
            } => {
                let share = proportion(nominal, budget, total);
                if share < minimum(nominal) {
                    minimum(nominal)
                } else {
                    *active += 1;
                    share + usize::from(*active <= leftover)
                }
            }
        }
    }
}

/// The layout of a widget for one width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Number of components kept: always the first ones.
    pub(crate) kept: usize,
    /// Index of the receiver of the extra columns, if it is kept and there are some.
    receiver: Option<usize>,
    /// Extra columns given to the receiver.
    extra: usize,
    /// How `L` and `I` get their width.
    shrink: Share,
    /// How the `N` get their width.
    names: Share,
}

/// Lay out the `len` components described by `item` in `width` columns. `receiver` is the
/// index of the component that takes the extra columns (the elastic `L`, otherwise the elastic
/// `I`), if any.
///
/// Linear in `len`: the sums of the widths are computed once for the whole composition, and
/// the last component is subtracted at each drop (step 5); only the final sharing goes over
/// the kept components again.
pub(crate) fn plan(
    len: usize,
    receiver: Option<usize>,
    width: usize,
    item: impl Fn(usize) -> Item,
) -> Plan {
    let mut sums = Sums::default();
    for i in 0..len {
        sums.add(item(i));
    }
    let mut kept = len;
    loop {
        if let Some(plan) = fit(kept, &sums, receiver, width, &item) {
            return plan;
        }
        // `fit` always succeeds with nothing kept (no width is needed): `kept` is positive.
        kept -= 1;
        sums.remove(item(kept));
    }
}

/// The sums `fit` needs over the kept components.
#[derive(Debug, Default, Clone, Copy)]
struct Sums {
    /// Widths of the fixed components.
    fixed: usize,
    /// Nominal widths of `L` and `I`.
    shrink_nominal: usize,
    /// Number of `L` and `I`.
    shrink_count: usize,
    /// Nominal widths of the `N`.
    names_nominal: usize,
    /// Minimum widths of the `N`.
    names_minimum: usize,
}

impl Sums {
    fn add(&mut self, item: Item) {
        match item {
            Item::Fixed(w) => self.fixed += w,
            Item::Shrink(w) => {
                self.shrink_nominal += w;
                self.shrink_count += 1;
            }
            Item::Name(w) => {
                self.names_nominal += w;
                self.names_minimum += minimum(w);
            }
        }
    }

    /// Undo [`Sums::add`] for a component added before.
    fn remove(&mut self, item: Item) {
        match item {
            Item::Fixed(w) => self.fixed -= w,
            Item::Shrink(w) => {
                self.shrink_nominal -= w;
                self.shrink_count -= 1;
            }
            Item::Name(w) => {
                self.names_nominal -= w;
                self.names_minimum -= minimum(w);
            }
        }
    }
}

/// The layout of the first `kept` components in `width` columns, if they fit (steps 1–4);
/// `sums` are the sums over these components.
fn fit(
    kept: usize,
    sums: &Sums,
    receiver: Option<usize>,
    width: usize,
    item: &impl Fn(usize) -> Item,
) -> Option<Plan> {
    let available = width.checked_sub(sums.fixed)?;
    // A receiver that is not kept gets nothing: `Widths` never reaches it.
    let mut plan = Plan {
        kept,
        receiver,
        extra: 0,
        shrink: Share::Nominal,
        names: Share::Nominal,
    };
    let shrink_minimum = sums.shrink_count * MIN_FLEXIBLE_WIDTH;
    let shrinks = move || {
        (0..kept).filter_map(move |i| match item(i) {
            Item::Shrink(w) => Some(w),
            _ => None,
        })
    };
    let names = move || {
        (0..kept).filter_map(move |i| match item(i) {
            Item::Name(w) => Some(w),
            _ => None,
        })
    };
    if available >= sums.shrink_nominal + sums.names_nominal {
        // Step 2: everything at its nominal width.
        plan.extra = available - sums.shrink_nominal - sums.names_nominal;
    } else if available >= sums.names_nominal + shrink_minimum {
        // Step 3: `L` and `I` shrink.
        plan.shrink = Share::proportional(available - sums.names_nominal, shrinks);
    } else if available >= shrink_minimum + sums.names_minimum {
        // Step 4: `L` and `I` at 2, the `N` shrink.
        plan.shrink = Share::Minimum;
        plan.names = Share::proportional(available - shrink_minimum, names);
    } else {
        return None;
    }
    Some(plan)
}

impl Plan {
    /// The widths of the kept components, in order (`item` as given to [`plan`]).
    pub(crate) fn widths<F: Fn(usize) -> Item>(&self, item: F) -> Widths<F> {
        Widths {
            plan: *self,
            item,
            index: 0,
            shrink_active: 0,
            names_active: 0,
        }
    }
}

/// Iterator of [`Plan::widths`].
pub(crate) struct Widths<F> {
    plan: Plan,
    item: F,
    index: usize,
    shrink_active: usize,
    names_active: usize,
}

impl<F: Fn(usize) -> Item> Iterator for Widths<F> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.index >= self.plan.kept {
            return None;
        }
        let index = self.index;
        self.index += 1;
        let extra = if self.plan.receiver == Some(index) {
            self.plan.extra
        } else {
            0
        };
        Some(match (self.item)(index) {
            Item::Fixed(w) => w,
            Item::Shrink(w) => self.plan.shrink.width(w, &mut self.shrink_active) + extra,
            Item::Name(w) => self.plan.names.width(w, &mut self.names_active),
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use proptest::prelude::*;

    /// The widths of the kept components of `items` in `width` columns.
    pub(crate) fn layout(items: &[Item], receiver: Option<usize>, width: usize) -> Vec<usize> {
        let item = |i: usize| items[i];
        plan(items.len(), receiver, width, item)
            .widths(item)
            .collect()
    }

    use Item::{Fixed, Name, Shrink};

    #[test]
    fn step_2_nominal_widths_and_the_receiver_takes_the_rest() {
        let items = [Fixed(3), Shrink(5), Fixed(1), Name(4)];
        assert_eq!(layout(&items, Some(1), 20), [3, 12, 1, 4]);
        assert_eq!(layout(&items, Some(1), 13), [3, 5, 1, 4]);
        // Without a receiver the widget keeps its natural width.
        assert_eq!(layout(&items, None, 20), [3, 5, 1, 4]);
    }

    #[test]
    fn step_3_l_and_i_shrink_in_proportion() {
        // L nominal 30, I nominal 10, budget 20: 15 and 5.
        let items = [Fixed(2), Shrink(30), Shrink(10)];
        assert_eq!(layout(&items, Some(1), 22), [2, 15, 5]);
        // Budget 21: 15.75 and 5.25 → 15 and 5, the rounding column to the first.
        assert_eq!(layout(&items, Some(1), 23), [2, 16, 5]);
    }

    #[test]
    fn step_3_a_member_held_at_2_leaves_the_sharing() {
        // Budget 12 for 100 and 10: 10.9 and 1.09 → I held at 2, L gets 10.
        let items = [Shrink(100), Shrink(10)];
        assert_eq!(layout(&items, Some(0), 12), [10, 2]);
    }

    #[test]
    fn step_3_names_keep_their_width() {
        let items = [Name(6), Shrink(20)];
        assert_eq!(layout(&items, Some(1), 10), [6, 4]);
    }

    #[test]
    fn step_4_names_shrink_once_l_and_i_are_at_2() {
        let items = [Name(10), Name(5), Shrink(20)];
        // A = 11: L at 2, 9 for the names: 6 and 3.
        assert_eq!(layout(&items, Some(2), 11), [6, 3, 2]);
        // A = 6: L at 2, 4 for the names: both at 2.
        assert_eq!(layout(&items, Some(2), 6), [2, 2, 2]);
    }

    #[test]
    fn step_4_rounding_columns_go_in_composition_order() {
        // 3 names of 3, budget 7: 2.33 each → 2, 2, 2 and one column to the first.
        let items = [Name(3), Name(3), Name(3)];
        assert_eq!(layout(&items, None, 7), [3, 2, 2]);
        assert_eq!(layout(&items, None, 8), [3, 3, 2]);
    }

    #[test]
    fn a_name_one_column_wide_never_grows_nor_shrinks() {
        let items = [Name(1), Name(10)];
        assert_eq!(layout(&items, None, 4), [1, 3]);
        assert_eq!(layout(&items, None, 3), [1, 2]);
    }

    #[test]
    fn step_5_drops_the_last_components() {
        let items = [Fixed(3), Shrink(10), Fixed(4)];
        assert_eq!(layout(&items, Some(1), 9), [3, 2, 4]);
        // 8 columns: 3 + 2 + 4 does not fit, the last is dropped and L takes the rest.
        assert_eq!(layout(&items, Some(1), 8), [3, 5]);
        assert_eq!(layout(&items, Some(1), 4), [3]);
        assert_eq!(layout(&items, Some(1), 3), [3]);
    }

    #[test]
    fn a_dropped_receiver_gets_nothing() {
        // 10 columns: the wide barrier does not fit, the receiver (index 3) after it is dropped
        // too, and the extra columns go to nobody.
        let items = [Shrink(2), Fixed(3), Fixed(20), Shrink(4)];
        assert_eq!(layout(&items, Some(3), 10), [2, 3]);
        // Shrinking comes before dropping.
        let items = [Shrink(10), Fixed(3), Shrink(4)];
        assert_eq!(layout(&items, Some(2), 14), [8, 3, 3]);
        assert_eq!(layout(&items, Some(2), 7), [2, 3, 2]);
        assert_eq!(layout(&items, Some(2), 6), [3, 3]);
    }

    #[test]
    fn empty_line_when_nothing_fits() {
        assert_eq!(
            layout(&[Fixed(3), Shrink(4)], Some(1), 2),
            Vec::<usize>::new()
        );
        assert_eq!(layout(&[Fixed(1)], None, 0), Vec::<usize>::new());
        assert_eq!(layout(&[Shrink(5)], Some(0), 1), Vec::<usize>::new());
        assert_eq!(layout(&[], None, 10), Vec::<usize>::new());
    }

    /// The previous `plan`, as a reference: the sums computed again for each prefix, from the
    /// longest down (quadratic in the number of components).
    fn reference_plan(items: &[Item], receiver: Option<usize>, width: usize) -> Plan {
        let item = |i: usize| items[i];
        (0..=items.len())
            .rev()
            .find_map(|kept| {
                let mut sums = Sums::default();
                for i in 0..kept {
                    sums.add(item(i));
                }
                fit(kept, &sums, receiver, width, &item)
            })
            .expect("nothing kept always fits")
    }

    #[test]
    fn the_layout_visits_each_component_a_few_times() {
        // 1 360 components (a 4 096-byte composition), most of them dropped.
        let items: Vec<Item> = (0..1360)
            .map(|i| match i % 4 {
                0 => Fixed(3),
                1 => Name(5 + i % 7),
                2 => Fixed(1),
                _ => Name(2),
            })
            .chain([Shrink(10), Shrink(30)])
            .collect();
        for width in [0, 80, 400, 2000, 10_000] {
            let calls = std::cell::Cell::new(0);
            let item = |i: usize| {
                calls.set(calls.get() + 1);
                items[i]
            };
            let plan = plan(items.len(), Some(items.len() - 1), width, item);
            let widths: usize = plan.widths(item).sum();
            assert!(widths <= width);
            assert!(
                calls.get() <= 8 * items.len(),
                "width {width}: {} calls for {} components",
                calls.get(),
                items.len()
            );
        }
    }

    /// Reference sharing, written as plainly as possible: share `budget` in proportion to the
    /// nominal widths; a member whose share falls below its minimum is held at the minimum,
    /// and the rest is shared again among the others until none falls below; then the
    /// leftover columns go one by one to the first members not held.
    fn reference_share(nominals: &[usize], budget: usize) -> Vec<usize> {
        let mut held = vec![false; nominals.len()];
        loop {
            let rest = budget
                - nominals
                    .iter()
                    .zip(&held)
                    .filter(|&(_, &h)| h)
                    .map(|(&w, _)| minimum(w))
                    .sum::<usize>();
            let total: usize = nominals
                .iter()
                .zip(&held)
                .filter(|&(_, &h)| !h)
                .map(|(&w, _)| w)
                .sum();
            let shares: Vec<usize> = nominals
                .iter()
                .map(|&w| (w * rest).checked_div(total).unwrap_or(0))
                .collect();
            let newly: Vec<usize> = (0..nominals.len())
                .filter(|&i| !held[i] && shares[i] < minimum(nominals[i]))
                .collect();
            if newly.is_empty() {
                let mut widths: Vec<usize> = (0..nominals.len())
                    .map(|i| {
                        if held[i] {
                            minimum(nominals[i])
                        } else {
                            shares[i]
                        }
                    })
                    .collect();
                let mut leftover = rest
                    - (0..nominals.len())
                        .filter(|&i| !held[i])
                        .map(|i| shares[i])
                        .sum::<usize>();
                for i in 0..nominals.len() {
                    if !held[i] && leftover > 0 {
                        widths[i] += 1;
                        leftover -= 1;
                    }
                }
                return widths;
            }
            for i in newly {
                held[i] = true;
            }
        }
    }

    fn items() -> impl Strategy<Value = (Vec<Item>, Option<usize>)> {
        let item = prop_oneof![
            (0usize..12).prop_map(Fixed),
            (2usize..40).prop_map(Shrink),
            (1usize..30).prop_map(Name),
        ];
        (
            proptest::collection::vec(item, 0..12),
            any::<proptest::sample::Index>(),
        )
            .prop_map(|(items, pick)| {
                let shrinks: Vec<usize> = (0..items.len())
                    .filter(|&i| matches!(items[i], Shrink(_)))
                    .collect();
                let receiver = (!shrinks.is_empty()).then(|| shrinks[pick.index(shrinks.len())]);
                (items, receiver)
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(10_000))]

        #[test]
        fn properties((items, receiver) in items(), width in 0usize..120) {
            let widths = layout(&items, receiver, width);
            prop_assert!(widths.iter().sum::<usize>() <= width);
            for (i, &w) in widths.iter().enumerate() {
                match items[i] {
                    Fixed(n) => prop_assert_eq!(w, n),
                    Shrink(n) | Name(n) => {
                        prop_assert!(w >= minimum(n));
                        if receiver != Some(i) {
                            prop_assert!(w <= n);
                        }
                    }
                }
            }
            // The next component (if any) did not fit.
            let more = layout(&items, receiver, width + 1);
            prop_assert!(more.len() >= widths.len());
            // Same plan as the previous, quadratic, algorithm.
            let item = |i: usize| items[i];
            prop_assert_eq!(plan(items.len(), receiver, width, item), reference_plan(&items, receiver, width));
        }

        #[test]
        fn sharing_matches_the_reference(
            nominals in proptest::collection::vec(1usize..50, 1..8),
            pick in 0.0f64..1.0,
        ) {
            let low: usize = nominals.iter().map(|&w| minimum(w)).sum();
            let high: usize = nominals.iter().sum();
            prop_assume!(low < high);
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "a small budget"
            )]
            let budget = low + ((high - low) as f64 * pick) as usize;
            let share = Share::proportional(budget, || nominals.iter().copied());
            let mut active = 0;
            let widths: Vec<usize> = nominals.iter().map(|&w| share.width(w, &mut active)).collect();
            prop_assert_eq!(&widths, &reference_share(&nominals, budget));
            prop_assert_eq!(widths.iter().sum::<usize>(), budget);
        }
    }
}
