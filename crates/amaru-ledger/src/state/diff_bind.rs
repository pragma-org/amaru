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

use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

/// A compact data-structure tracking changes in a DAG which supports optional linking of values with
/// another data-structure. Items can only be linked if they have been registered first. Yet, they
/// can be unlinked without being unregistered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffBind<K: Ord, L, R, V> {
    pub registered: BTreeMap<K, Bind<L, R, V>>,
    pub unregistered: BTreeSet<K>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind<L, R, V> {
    pub left: Option<L>,
    pub right: Option<R>,
    pub value: Option<V>,
}

pub struct Empty;

impl<K: Ord, L, R, V> Default for DiffBind<K, L, R, V> {
    fn default() -> Self {
        Self {
            registered: Default::default(),
            unregistered: Default::default(),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Key is already registered")]
    KeyAlreadyRegistered,
    #[error("Key is already unregistered")]
    KeyAlreadyUnregistered,
}

impl<K: Ord, L, R, V> DiffBind<K, L, R, V> {
    pub fn register(
        &mut self,
        key: K,
        value: V,
        left: Option<L>,
        right: Option<R>,
    ) -> Result<(), Error> {
        if self.registered.contains_key(&key) {
            return Err(Error::KeyAlreadyRegistered);
        }

        self.unregistered.remove(&key);
        self.registered.insert(
            key,
            Bind {
                left,
                right,
                value: Some(value),
            },
        );

        Ok(())
    }

    pub fn bind_left(&mut self, key: K, left: Option<L>) -> Result<(), Error> {
        if self.unregistered.contains(&key) {
            return Err(Error::KeyAlreadyUnregistered);
        }

        match self.registered.entry(key) {
            Entry::Occupied(mut e) => {
                e.get_mut().left = left;
            }
            Entry::Vacant(e) => {
                e.insert(Bind {
                    left,
                    right: None,
                    value: None,
                });
            }
        }

        Ok(())
    }

    pub fn bind_right(&mut self, key: K, right: Option<R>) -> Result<(), Error> {
        if self.unregistered.contains(&key) {
            return Err(Error::KeyAlreadyUnregistered);
        }

        match self.registered.entry(key) {
            Entry::Occupied(mut e) => {
                e.get_mut().right = right;
            }
            Entry::Vacant(e) => {
                e.insert(Bind {
                    left: None,
                    right,
                    value: None,
                });
            }
        }

        Ok(())
    }

    pub fn unregister(&mut self, key: K) {
        self.registered.remove(&key);
        self.unregistered.insert(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_some_left_then_bind_left() {
        let mut diff_bind = DiffBind::default();
        diff_bind
            .register(1, "value", Some("left_1"), None::<()>)
            .unwrap();
        diff_bind.bind_left(1, Some("left_2")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: Some("left_2"),
                right: None,
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_left_then_bind_right() {
        let mut diff_bind = DiffBind::default();
        diff_bind
            .register(1, "value", None::<()>, Some("right_1"))
            .unwrap();
        diff_bind.bind_right(1, Some("right_2")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: None,
                right: Some("right_2"),
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_left_then_unbind_left() {
        let mut diff_bind = DiffBind::default();
        diff_bind
            .register(1, "value", Some("left"), None::<()>)
            .unwrap();
        diff_bind.bind_left(1, None).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: None,
                right: None,
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_some_right_then_unbind_right() {
        let mut diff_bind = DiffBind::default();
        diff_bind
            .register(1, "value", None::<()>, Some("right"))
            .unwrap();
        diff_bind.bind_right(1, None).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: None,
                right: None,
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_then_unregister() {
        let mut diff_bind = DiffBind::default();
        diff_bind
            .register(1, "value", None::<()>, None::<()>)
            .unwrap();
        diff_bind.unregister(1);
        assert!(diff_bind.unregistered.contains(&1));
        assert!(diff_bind.registered.is_empty());
    }

    #[test]
    fn register_none_then_bind_left() {
        let mut diff_bind = DiffBind::default();
        diff_bind.register(1, "value", None, None::<()>).unwrap();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: Some("left"),
                right: None,
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_none_then_bind_right() {
        let mut diff_bind = DiffBind::default();
        diff_bind.register(1, "value", None::<()>, None).unwrap();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: None,
                right: Some("right"),
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn register_none_then_bind_left_and_right() {
        let mut diff_bind = DiffBind::default();
        diff_bind.register(1, "value", None, None).unwrap();
        diff_bind.bind_left(1, Some("left")).unwrap();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: Some("left"),
                right: Some("right"),
                value: Some("value")
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn bind_left_then_register_fails() {
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None, None::<()>),
            Err(Error::KeyAlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_right_then_register_fails() {
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(matches!(
            diff_bind.register(1, "value", None::<()>, None),
            Err(Error::KeyAlreadyRegistered { .. })
        ));
    }

    #[test]
    fn bind_left_only() {
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_left(1, Some("left")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: Some("left"),
                right: None::<()>,
                value: None::<()>
            }),
            diff_bind.registered.get(&1)
        );
    }

    #[test]
    fn bind_right_only() {
        let mut diff_bind = DiffBind::default();
        diff_bind.bind_right(1, Some("right")).unwrap();
        assert!(diff_bind.unregistered.is_empty());
        assert!(diff_bind.registered.contains_key(&1));
        assert_eq!(
            Some(&Bind {
                left: None::<()>,
                right: Some("right"),
                value: None::<()>
            }),
            diff_bind.registered.get(&1)
        );
    }
}
