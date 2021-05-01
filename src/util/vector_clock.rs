//! Private module for selective re-export.

use std::cmp::{max, Ordering};
use std::hash::{Hash, Hasher};
use std::fmt::{self, Display, Formatter};

/// A [vector clock](https://en.wikipedia.org/wiki/Vector_clock), which provides a partial causal
/// order on events in a distributed sytem.
#[derive(Clone, Debug, Default, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct VectorClock(Vec<u32>);

impl VectorClock {
    /// Instantiates a vector clock.
    pub fn new() -> Self {
        VectorClock(Vec::new())
    }

    /// Creates a new vector clock that merges two other vector clocks by picking the maximum
    /// clock component of each corresponding element.
    pub fn merge_max(c1: &VectorClock, c2: &VectorClock) -> Self {
        let VectorClock(c1) = c1;
        let VectorClock(c2) = c2;
        let mut result = vec![0; max(c1.len(), c2.len())];
        for (i, element) in result.iter_mut().enumerate() {
            let v1 = *c1.get(i).unwrap_or(&0);
            let v2 = *c2.get(i).unwrap_or(&0);
            *element = max(v1, v2);
        }
        VectorClock(result)
    }

    /// Creates a new vector clock that increments a component of a previous vector clock.
    pub fn incremented(mut self, index: usize) -> Self {
        if index >= self.0.len() {
            self.0.resize(1 + index, 0);
        }
        self.0[index] += 1;
        self
    }
}

impl Display for VectorClock {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "<")?;
        for c in &self.0 {
            write!(f, "{}, ", c)?;
        }
        write!(f, "...>")?;
        Ok(())
    }
}

impl Hash for VectorClock {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let cutoff = self.0.iter()
            .rposition(|elem| elem != &0)
            .map(|i| i + 1)
            .unwrap_or(0);
        self.0[..cutoff].hash(state);
    }
}

impl PartialEq for VectorClock {
    fn eq(&self, rhs: &Self) -> bool {
        for i in 0..max(self.0.len(), rhs.0.len()) {
            let lhs_elem = self.0.get(i).unwrap_or(&0);
            let rhs_elem = rhs.0.get(i).unwrap_or(&0);
            if lhs_elem != rhs_elem {
                return false
            }
        }
        true
    }
}

impl From<Vec<u32>> for VectorClock {
    fn from(v: Vec<u32>) -> Self {
        VectorClock(v)
    }
}

impl PartialOrd for VectorClock {
    fn partial_cmp(&self, rhs: &Self) -> Option<Ordering> {
        let mut expected_ordering = Ordering::Equal;
        for i in 0..max(self.0.len(), rhs.0.len()) {
            let ordering = {
                let lhs_elem = self.0.get(i).unwrap_or(&0);
                let rhs_elem = rhs.0.get(i).unwrap_or(&0);
                lhs_elem.cmp(&rhs_elem)
            };

            // The algorithm starts by expecting the vectors to be equal. Once it finds a
            // `Less`/`Greater` element, it expects all other elements to be that or `Equal`
            // (e.g. `[1, *2*, ...]` vs `[1, *3*, ...]` would switch from `Equal` to `Less`).
            // If the ordering switches again then the vectors are incomparable (e.g. continuing
            // the earlier example `[1, 2, *4*]` vs `[1, 3, 0]` would return `None`).
            if expected_ordering == Ordering::Equal {
                expected_ordering = ordering;
            } else if ordering != expected_ordering && ordering != Ordering::Equal {
                return None
            }
        }
        Some(expected_ordering)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn can_display() {
        assert_eq!(
            format!("{}", VectorClock::from(vec![1, 2, 3, 4])),
            "<1, 2, 3, 4, ...>");

        // Notably equal vectors don't necessarily display the same.
        assert_eq!(
            format!("{}", VectorClock::from(vec![])),
            "<...>");
        assert_eq!(
            format!("{}", VectorClock::from(vec![0])),
            "<0, ...>");
    }

    #[test]
    fn can_equate() {
        assert_eq!(
            VectorClock::new(),
            VectorClock::new());
        assert_eq!(
            VectorClock::from(vec![0]),
            VectorClock::from(vec![]));
        assert_eq!(
            VectorClock::from(vec![]),
            VectorClock::from(vec![0]));

        assert_ne!(
            VectorClock::from(vec![]),
            VectorClock::from(vec![1]));
        assert_ne!(
            VectorClock::from(vec![1]),
            VectorClock::from(vec![]));
    }

    #[test]
    fn can_hash() {
        use std::collections::hash_map::DefaultHasher;

        macro_rules! assert_hash_eq {
            ($v1:expr, $v2:expr) => {
                let mut h1 = DefaultHasher::new();
                let mut h2 = DefaultHasher::new();
                $v1.hash(&mut h1);
                $v2.hash(&mut h2);
                assert_eq!(h1.finish(), h2.finish());
            }
        }
        macro_rules! assert_hash_ne {
            ($v1:expr, $v2:expr) => {
                let mut h1 = DefaultHasher::new();
                let mut h2 = DefaultHasher::new();
                $v1.hash(&mut h1);
                $v2.hash(&mut h2);
                assert_ne!(h1.finish(), h2.finish());
            }
        }

        // same hash if equal
        assert_hash_eq!(
            VectorClock::new(),
            VectorClock::new());
        assert_hash_eq!(
            VectorClock::from(vec![]),
            VectorClock::from(vec![0, 0]));
        assert_hash_eq!(
            VectorClock::from(vec![1]),
            VectorClock::from(vec![1, 0]));

        // otherwise hash varies w/ high probability
        assert_hash_ne!(
            VectorClock::from(vec![]),
            VectorClock::from(vec![1]));
        assert_hash_ne!(
            VectorClock::from(vec![1]),
            VectorClock::from(vec![]));
    }

    #[test]
    fn can_increment() {
        assert_eq!(
            VectorClock::new().incremented(2),
            VectorClock::from(vec![0, 0, 1]));
        assert_eq!(
            VectorClock::new().incremented(2).incremented(0).incremented(2),
            VectorClock::from(vec![1, 0, 2]));
    }

    #[test]
    fn can_merge() {
        assert_eq!(
            VectorClock::merge_max(
                &VectorClock::from(vec![1, 2, 3, 4]),
                &VectorClock::from(vec![5, 6, 0])),
            VectorClock::from(vec![5, 6, 3, 4]));
        assert_eq!(
            VectorClock::merge_max(
                &VectorClock::from(vec![1, 0, 2]),
                &VectorClock::from(vec![3, 1, 0, 4])),
            VectorClock::from(vec![3, 1, 2, 4]));
    }

    #[test]
    fn can_order_partially() {
        use Ordering::*;

        // Clocks with matching elements are equal. Missing elements are implicitly zero.
        assert_eq!(
            Some(Equal),
            VectorClock::from(vec![]).partial_cmp(&vec![].into()));
        assert_eq!(
            Some(Equal),
            VectorClock::from(vec![]).partial_cmp(&vec![0, 0].into()));
        assert_eq!(
            Some(Equal),
            VectorClock::from(vec![0, 0]).partial_cmp(&vec![].into()));
        assert_eq!(
            Some(Equal),
            VectorClock::from(vec![1, 2, 0]).partial_cmp(&vec![1, 2].into()));

        // A clock is less if at least one element is less and the rest are
        // less-than-or-equal.
        assert_eq!(
            Some(Less),
            VectorClock::from(vec![]).partial_cmp(&vec![1].into()));
        assert_eq!(
            Some(Less),
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![1, 3, 4].into()));
        assert_eq!(
            Some(Less),
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![1, 3, 3].into()));
        assert_eq!(
            Some(Less),
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![2, 3, 3].into()));

        // A clock is greater if at least one element is greater and the rest are
        // greater-than-or-equal.
        assert_eq!(
            Some(Greater),
            VectorClock::from(vec![1]).partial_cmp(&vec![].into()));
        assert_eq!(
            Some(Greater),
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![1, 1, 2].into()));
        assert_eq!(
            Some(Greater),
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![1, 1, 3].into()));
        assert_eq!(
            Some(Greater),
            VectorClock::from(vec![1, 2, 4]).partial_cmp(&vec![0, 1, 3].into()));

        // If one element is greater while another is less, then the vectors are incomparable.
        assert_eq!(
            None,
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![1, 3, 2].into()));
        assert_eq!(
            None,
            VectorClock::from(vec![1, 2, 3]).partial_cmp(&vec![3, 2, 1].into()));
        assert_eq!(
            None,
            VectorClock::from(vec![1, 2, 2]).partial_cmp(&vec![2, 1, 2].into()));
    }
}
