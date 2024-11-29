use crate::ledger::kernel::Epoch;
use std::collections::BTreeMap;
use vec1::{vec1, Vec1};

/// A compact data-structure tracking deferred registration & unregistration changes in a key:value
/// store. By deferred, we reflect on the fact that unregistering a value isn't immediate, but
/// occurs only after a certain epoch (specified when unregistering). Similarly, re-registering is
/// treated as an update, but always deferred to some specified epoch as well.
///
/// The data-structure can be reduced through a composition relation that ensures two
/// `DiffEpochReg` collapses into one that is equivalent to applying both `DiffEpochReg` in
/// sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEpochReg<K, V> {
    pub registered: BTreeMap<K, Vec1<V>>,
    pub unregistered: BTreeMap<K, Epoch>,
}

impl<K, V> Default for DiffEpochReg<K, V> {
    fn default() -> Self {
        Self {
            registered: Default::default(),
            unregistered: Default::default(),
        }
    }
}

impl<K: Ord, V> DiffEpochReg<K, V> {
    /// We reduce registration and de-registration according to the following rules:
    ///
    /// 1. A single `DiffEpochReg` spans over *a block*. Thus, there is no epoch change whatsoever
    ///    happening within a single block.
    ///
    /// 2. Beyond the first registration, any new registration takes precedence. Said differently,
    ///    there's always _at most_ two registrations.
    ///
    ///    In practice, the first registation could also *sometimes* be collapsed, if there's
    ///    already a registration in the stable storage. But we don't have acccess to the storage
    ///    here, so by default, we'll always keep the first registration untouched.
    ///
    /// 3. Registration immediately cancels out any unregistration.
    ///
    /// 4. There can be at most 1 unregistration per entity. Any new unregistration is preferred
    ///    and replaces previous registrations.
    pub fn register(&mut self, k: K, v: V) {
        self.unregistered.remove(&k);
        match self.registered.get_mut(&k) {
            None => {
                self.registered.insert(k, vec1![v]);
            }
            Some(vs) => {
                if vs.len() > 1 {
                    vs[1] = v;
                } else {
                    vs.push(v);
                }
            }
        }
    }

    // See 'register' for details.
    pub fn unregister(&mut self, k: K, epoch: Epoch) {
        self.unregistered.insert(k, epoch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::BTreeMap;

    prop_compose! {
        fn any_diff()(
            registered in
                any::<BTreeMap<u8, Vec<u8>>>(),
            unregistered in
                any::<BTreeMap<u8, Epoch>>()
        ) -> DiffEpochReg<u8, u8> {
            DiffEpochReg {
                registered: registered
                    .into_iter()
                    .filter_map(|(k, mut v)| {
                        v.truncate(2);
                        let v = Vec1::try_from(v).ok()?;
                        Some((k, v))
                    })
                    .collect::<BTreeMap<_, _>>(),
                unregistered,
            }
        }
    }

    proptest! {
        // NOTE: We could avoid this test altogether by modelling the type in a different way.
        // Having a sum One(V) | Two(V, V) instead of a Vec1 would give us this guarantee _by
        // construction_.
        #[test]
        fn prop_register(mut st in any_diff(), (k, v) in any::<(u8, u8)>()) {
            st.register(k, v);
            let vs = st.registered.get(&k).expect("we just registered an element");
            assert!(vs.len() <= 2, "registered[{k}] = {:?} has more than 2 elements", vs);
            if vs.len() == 1 {
                assert_eq!(vs, &vec1![v], "only element is different");
            } else {
                assert_eq!(*vs.last(), v, "last element is different");
            }
        }
    }

    proptest! {
        #[test]
        fn prop_register_cancels_unregister(mut st in any_diff(), (k, v) in any::<(u8, u8)>()) {
            st.register(k, v);
            assert!(!st.unregistered.contains_key(&k))
        }
    }

    proptest! {
        #[test]
        fn prop_unregister_right_biaised(mut st in any_diff(), (k, e) in any::<(u8, Epoch)>()) {
            st.unregister(k, e);
            let e_retained = st.unregistered.get(&k);
            assert_eq!(e_retained, Some(&e))
        }
    }
}
