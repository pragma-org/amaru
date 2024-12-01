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

/// Captures the outcome of folding over a sequence of DiffEpochReg. The 'Undetermined' case
/// indicates that it isn't possible to conclude anything from the sequence and more information
/// (from the stable storage) is needed to obtain the current state of the item `V`.
#[derive(Debug, PartialEq)]
pub enum Fold<'a, V> {
    Registered(&'a V),
    Unregistered,
    Undetermined,
}

impl<'a, V> Fold<'a, V> {
    /// View the result of a sequence of 'DiffEpochReg' from a given epoch and for a particular
    /// key. This is used to determine whether the current sequence of diffs is enough to know the
    /// current (as per the given epoch) state of the tracked values.
    pub fn for_epoch<K: Ord + 'a>(
        epoch: Epoch,
        key: &K,
        iterator: impl Iterator<Item = (Epoch, &'a DiffEpochReg<K, V>)>,
    ) -> Self {
        let fold = iterator.fold(DiffEpochReg::default(), |mut state, step| {
            if step.0 < epoch {
                if let Some(registrations) = step.1.registered.get(key) {
                    state.register(key, registrations.last());
                }

                if let Some(retirement) = step.1.unregistered.get(key) {
                    if retirement <= &epoch {
                        state.unregister(key, *retirement);
                    }
                }
            }

            state
        });

        if fold.unregistered.contains_key(key) {
            return Fold::Unregistered;
        }

        if let Some(registrations) = fold.registered.get(key) {
            return Fold::Registered(registrations.last());
        }

        Fold::Undetermined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::{btree_map, BTreeMap};

    pub const MAX_EPOCH: Epoch = 4;

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

    #[derive(Debug, Clone)]
    pub enum Message<K, V> {
        Register(K, V),
        Unregister(K, Epoch),
    }

    prop_compose! {
        fn any_message(max_epoch: Epoch)(
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
        prop::collection::vec(0..MAX_EPOCH, 1..30).prop_flat_map(move |mut epochs| {
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
                                        Message::Unregister(k, 1 + epoch + offset)
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
                assert!(epoch <= MAX_EPOCH);
                assert!(epoch >= current_epoch);
                epoch
            });
        }
    }

    proptest! {
        #[test]
        fn prop_equivalent_to_simpler_model(msgs in any_message_sequence()) {
            #[derive(Debug, Default)]
            struct Model {
                epoch: Epoch,
                current: BTreeMap<char, u8>,
                future: BTreeMap<char, u8>,
                retiring: BTreeMap<char, Epoch>,
            }

            // Process messages through the model implementation
            let model = msgs.iter().fold(Model::default(), |mut model, (epoch, blk)| {
                // Apply transition rules on epoch boundary.
                if epoch > &model.epoch {
                    model.current.append(&mut model.future);
                    let mut ks = Vec::new();
                    for (k, retirement) in model.retiring.iter() {
                        if retirement <= epoch {
                            model.current.remove(k);
                            ks.push(*k);
                        }
                    }
                    for k in ks {
                        model.retiring.remove(&k);
                    }
                }

                for msg in blk {
                    match msg {
                        Message::Register(k, v) => {
                            model.retiring.remove(k);
                            if let btree_map::Entry::Vacant(entry) = model.current.entry(*k)  {
                                entry.insert(*v);
                            } else {
                                model.future.insert(*k, *v);
                            }
                        }
                        Message::Unregister(k, e) => {
                            model.retiring.insert(*k, *e);
                        }
                    }
                }

                model
            });

            // Process messages through the real implementation
            let real = msgs.iter().fold(Vec::new(), |mut real, (epoch, blk)| {
                let mut diff = DiffEpochReg::default();
                for msg in blk {
                    match msg {
                        Message::Register(k, v) => diff.register(*k, *v),
                        Message::Unregister(k, e) => diff.unregister(*k, *e),
                    }
                }
                real.push((*epoch, diff));
                real
            });

            // Compare real & model
            for (k, v) in model.current.iter() {
                let fold = Fold::for_epoch(
                    model.epoch,
                    k,
                    real.iter().map(|(epoch, diff)| (*epoch, diff))
                );

                // NOTE: when we only register a value once, the real implementation cannot
                // properly determine whether its a genuine new registration or a re-registration.
                if fold != Fold::Undetermined {
                    assert_eq!(
                        fold,
                        Fold::Registered(v),
                        "model = {model:?}\nreal = {real:?}"
                    );
                }
            }

            for (k, e) in model.retiring.iter() {
                let fold = Fold::for_epoch(
                    model.epoch,
                    k,
                    real.iter().map(|(epoch, diff)| (*epoch, diff))
                );

                assert_eq!(
                    fold,
                    Fold::Undetermined,
                    "model = {model:?}\nreal = {real:?}"
                );

                let fold = Fold::for_epoch(
                    *e,
                    k,
                    real.iter().map(|(epoch, diff)| (*epoch, diff))
                );

                assert_eq!(
                    fold,
                    Fold::Unregistered,
                    "model = {model:?}\nreal = {real:?}"
                );
            }
        }
    }
}
