// Copyright 2024 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::{collections::BTreeMap, ops::Deref};

use amaru_kernel::Epoch;

use crate::state::volatile::Registrations;

/// A compact data-structure tracking deferred registration & unregistration changes in a key:value
/// store. By deferred, we reflect on the fact that unregistering a value isn't immediate, but
/// occurs only after a certain epoch (specified when unregistering). Similarly, re-registering is
/// treated as an update, but always deferred to some specified epoch as well.
///
/// The data-structure can be reduced through a composition relation that ensures two
/// `DiffEpochReg` collapses into one that is equivalent to applying both `DiffEpochReg` in
/// sequence.
///
/// /!\ Important /!\
/// In its current state, it is NOT possible to reduce/merge DiffEpochReg *across epochs*. Calls to
/// `.register` and `.unregister` assumes they are all done from within the same epoch. Merging
/// across epochs requires some more finesse; which isn't completely out of the picture, but simply
/// hasn't been implemented yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEpochReg<K, V> {
    pub registered: BTreeMap<K, Registrations<V>>,
    pub unregistered: BTreeMap<K, Epoch>,
}

impl<K, V> Default for DiffEpochReg<K, V> {
    fn default() -> Self {
        Self { registered: Default::default(), unregistered: Default::default() }
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
                self.registered.insert(k, Registrations::new(v));
            }
            Some(registration) => registration.next(v),
        }
    }

    /// See [`Self::register`] for details.
    pub fn unregister(&mut self, k: K, epoch: Epoch) {
        self.unregistered.insert(k, epoch);
    }
}

impl<K: Ord + Copy, V> DiffEpochReg<K, V> {
    /// Merge two states together, assuming that the other state is the most recent.
    ///
    /// # Warning
    ///
    /// Both states MUST belong to the same epoch. This isn't suitable for combining states across
    /// epoch boundaries.
    pub fn append(&mut self, most_recent: Self) {
        self.append_with(most_recent.registered, most_recent.unregistered, std::convert::identity);
    }

    /// Internal helper for merging two `DiffEpochReg` with different ownership strategy for `V`
    /// (a.k.a `U`).
    fn append_with<U>(
        &mut self,
        registered: impl IntoIterator<Item = (K, Registrations<U>)>,
        unregistered: impl IntoIterator<Item = (K, Epoch)>,
        mut with_ownership_strategy: impl FnMut(U) -> V,
    ) {
        for (k, registrations) in registered {
            for value in registrations {
                self.register(k, with_ownership_strategy(value));
            }
        }

        for (k, epoch) in unregistered {
            self.unregister(k, epoch);
        }
    }
}

impl<'a, K: Ord + Copy, T> DiffEpochReg<K, &'a T> {
    /// Like [`DiffEpochReg::append`], but only requires derefs of values.
    pub fn append_derefs<V>(&mut self, most_recent: &'a DiffEpochReg<K, V>)
    where
        V: Deref<Target = T>,
    {
        self.append_with(
            most_recent.registered.iter().map(|(k, v)| (*k, v.as_derefs())),
            most_recent.unregistered.iter().map(|(k, v)| (*k, *v)),
            std::convert::identity,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;

    pub const MAX_EPOCH: u64 = 4;

    prop_compose! {
        fn any_diff()(
            registered in
                any::<BTreeMap<u8, (u8, Option<u8>)>>(),
            unregistered in
                any::<BTreeMap<u8, Epoch>>()
        ) -> DiffEpochReg<u8, u8> {
            DiffEpochReg {
                registered: registered
                    .into_iter()
                    .map(|(k, (current, next))| {
                        let mut registrations = Registrations::new(current);
                        if let Some(next) = next {
                            registrations.next(next);
                        }
                        (k, registrations)
                    })
                    .collect(),
                unregistered,
            }
        }
    }

    proptest! {
        // NOTE: We could avoid this test altogether by modelling the type in a different way.
        // Having a sum One(V) | Two(V, V) instead of a Vec would give us this guarantee _by
        // construction_.
        #[test]
        fn prop_register(mut st in any_diff(), (k, v) in any::<(u8, u8)>()) {
            st.register(k, v);
            let registrations = st.registered.get(&k).expect("we just registered an element");
            assert_eq!(registrations.last(), &v, "last element is different");
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

    #[derive(Debug, Clone)]
    pub enum Message<K, V> {
        Register(K, V),
        Unregister(K, u64),
    }

    prop_compose! {
        fn any_message(max_epoch: u64)(
            k in
                prop_oneof![Just('a'), Just('b'), Just('c')],
            v in
                any::<u8>(),
            epoch in
                prop_oneof![Just(None), (0..max_epoch).prop_map(Some)]
        ) -> Message<char, u8> {
            match epoch {
                None => Message::Register(k, v),
                Some(epoch) => Message::Unregister(k, epoch),
            }
        }

    }

    fn any_message_sequence() -> impl Strategy<Value = Vec<(Epoch, Vec<Message<char, u8>>)>> {
        let any_block = || prop::collection::vec(any_message(MAX_EPOCH), 0..5);
        prop::collection::vec(0..MAX_EPOCH, 1..30).prop_flat_map(move |epochs| {
            let mut epochs: Vec<Epoch> = epochs.into_iter().map(Epoch::from).collect();
            epochs.sort();
            prop::collection::vec(any_block(), epochs.len()).prop_map(move |msgs| {
                epochs
                    .iter()
                    .cloned()
                    .zip(msgs)
                    .map(|(epoch, blk)| {
                        (
                            epoch,
                            blk.into_iter()
                                .map(|msg| {
                                    if let Message::Unregister(k, offset) = msg {
                                        Message::Unregister(k, (epoch + offset + 1).into())
                                    } else {
                                        msg
                                    }
                                })
                                .collect::<Vec<_>>(),
                        )
                    })
                    .collect()
            })
        })
    }

    proptest! {
        #[test]
        fn prop_messages_are_in_ascending_epoch(msgs in any_message_sequence()) {
            msgs.into_iter().fold(0, |current_epoch, (epoch, _)| {
                assert!(epoch <= Epoch::from(MAX_EPOCH));
                assert!(epoch >= Epoch::from(current_epoch));
                epoch.into()
            });
        }
    }
}
