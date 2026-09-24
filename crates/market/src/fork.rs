use std::collections::{BTreeMap, HashSet};

use domain::Slot;

/// Parent links for slots above the last confirmed one, fed by one shard's
/// slot stream.
#[derive(Debug, Default)]
pub(crate) struct SlotTree {
    parents: BTreeMap<Slot, Slot>,
    confirmed: Option<Slot>,
    first_seen: Option<Slot>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resolution {
    pub confirmed: Slot,
    pub previous: Option<Slot>,
    canonical: HashSet<Slot>,
    /// The parent walk stopped at this slot without reaching `previous`;
    /// anything below it cannot be classified.
    unresolved_below: Option<Slot>,
    first_seen: Option<Slot>,
}

impl Resolution {
    /// Unclassifiable slots count as canonical, which is exactly what the
    /// store did before fork tracking, so a missing parent never loses data.
    pub fn is_canonical(&self, slot: Slot) -> bool {
        self.canonical.contains(&slot) || self.unresolved_below.is_some_and(|u| slot < u)
    }

    /// Slots created before the subscription started never had their parent
    /// sent to us, so a hole below the first parent we saw is expected.
    pub fn has_gap(&self) -> bool {
        self.previous.is_some()
            && self
                .unresolved_below
                .zip(self.first_seen)
                .is_some_and(|(missing, first)| missing > first)
    }
}

impl SlotTree {
    pub fn record_parent(&mut self, slot: Slot, parent: Slot) {
        self.first_seen.get_or_insert(slot);
        if self.confirmed.is_none_or(|c| slot > c) {
            self.parents.insert(slot, parent);
        }
    }

    /// `None` when `slot` is not newer than the last confirmed slot: its
    /// ancestors were already resolved by a later confirmation.
    pub fn confirm(&mut self, slot: Slot) -> Option<Resolution> {
        let previous = self.confirmed;
        if previous.is_some_and(|c| slot <= c) {
            return None;
        }
        let mut canonical = HashSet::new();
        let mut cursor = slot;
        let unresolved_below = loop {
            canonical.insert(cursor);
            match self.parents.get(&cursor) {
                Some(&parent) if previous.is_none_or(|c| parent > c) => cursor = parent,
                Some(_) => break None,
                None => break Some(cursor),
            }
        };
        self.confirmed = Some(slot);
        self.parents = self.parents.split_off(&Slot(slot.0 + 1));
        Some(Resolution {
            confirmed: slot,
            previous,
            canonical,
            unresolved_below,
            first_seen: self.first_seen,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_excludes_sibling_fork() {
        let mut tree = SlotTree::default();
        tree.confirm(Slot(100));
        tree.record_parent(Slot(101), Slot(100));
        tree.record_parent(Slot(102), Slot(100));
        tree.record_parent(Slot(103), Slot(101));
        let res = tree.confirm(Slot(103)).unwrap();
        assert_eq!(
            [101, 102, 103].map(|s| res.is_canonical(Slot(s))),
            [true, false, true]
        );
    }
}
