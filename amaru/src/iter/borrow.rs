use mutable_db_entry::MutableDBEntry;
use pallas_codec::minicbor as cbor;
use std::{borrow::BorrowMut, cell::RefCell, rc::Rc};

/// An iterator over borrowable items. This allows to define a Rust-idiomatic API for accessing
/// items in read-only or read-write mode. When provided in a callback, it allows the callee to
/// then perform specific operations (e.g. database updates) on items that have been mutably
/// borrowed.
pub type IterBorrow<'a, 'b, T> = Box<dyn Iterator<Item = Box<dyn BorrowMut<T> + 'a>> + 'b>;

pub fn new<'a, T: Clone>(
    inner: impl Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a,
) -> KeyValueIterator<'a, T> {
    KeyValueIterator {
        inner: Box::new(inner),
        updates: Rc::new(RefCell::new(Vec::new())),
    }
}

/// A KeyValueIterator defines an abstraction on top of another iterator, that stores updates applied to
/// iterated elements. This allows, for example, to iterate over elements of a key/value store,
/// collect mutations to the underlying value, and persist those mutations without leaking the
/// underlying store implementation to the caller.
pub struct KeyValueIterator<'a, T: Clone> {
    updates: SharedVec<(Vec<u8>, Option<T>)>,
    inner: RawIterator<'a>,
}

pub type RawIterator<'a> = Box<dyn Iterator<Item = (Box<[u8]>, Box<[u8]>)> + 'a>;

pub type SharedVec<T> = Rc<RefCell<Vec<T>>>;

impl<'a, T: Clone> KeyValueIterator<'a, T> {
    /// Obtain an iterator on the updates to be done. This takes ownership of the original
    /// iterator to ensure that it is correctly de-allocated as we now process updates.
    pub fn into_iter_updates(self) -> impl Iterator<Item = (Vec<u8>, Option<T>)> {
        // NOTE: In principle, 'into_iter_updates' is only called once all callbacks on the inner
        // iterator have resolved; so the absolute count of strong references should be 1 and no
        // cloning should occur here. We can prove that with the next assertion.
        assert_eq!(Rc::strong_count(&self.updates), 1);
        Rc::unwrap_or_clone(self.updates).into_inner().into_iter()
    }
}

impl<'a, T: Clone + for<'d> cbor::Decode<'d, ()>> KeyValueIterator<'a, T>
where
    Self: 'a,
{
    pub fn as_iter_borrow(&mut self) -> IterBorrow<'a, '_, Option<T>> {
        Box::new(self)
    }
}

impl<'a, T: Clone + for<'d> cbor::Decode<'d, ()>> Iterator for KeyValueIterator<'a, T>
where
    Self: 'a,
{
    type Item = Box<dyn BorrowMut<Option<T>> + 'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(k, v)| {
            let updates = self.updates.clone();
            let original: T = cbor::decode(&v)
                .unwrap_or_else(|e| panic!("unable to decode object ({}): {e:?}", hex::encode(&v)));
            Box::new(MutableDBEntry::new(Some(original), move |new| {
                updates
                    .as_ref()
                    .borrow_mut()
                    .push((k.to_vec(), new.clone()))
            })) as Self::Item
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pallas_codec::minicbor as cbor;
    use rocksdb::OptimisticTransactionDB;

    #[derive(Debug, Clone, PartialEq)]
    struct Fruit {
        name: String,
        quantity: usize,
    }

    impl<C> cbor::encode::Encode<C> for Fruit {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            e.array(2)?;
            e.encode_with(&self.name, ctx)?;
            e.encode_with(self.quantity, ctx)?;
            Ok(())
        }
    }

    impl<'b, C> cbor::decode::Decode<'b, C> for Fruit {
        fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
            let _len = d.array()?;
            Ok(Fruit {
                name: d.decode_with(ctx)?,
                quantity: d.decode_with(ctx)?,
            })
        }
    }

    /// A simple helper function to encode any (serialisable) value to CBOR bytes.
    fn to_cbor<T: cbor::Encode<()> + std::fmt::Debug>(value: T) -> Vec<u8> {
        let mut buffer = Vec::new();
        cbor::encode(value, &mut buffer)
            .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
        buffer
    }

    #[test]
    fn db_iterator_mutate() {
        let db: OptimisticTransactionDB =
            OptimisticTransactionDB::open_default(std::env::temp_dir()).unwrap();

        // Insert some values in a prefixed colletion
        db.put(
            "fruit:apple",
            to_cbor(Fruit {
                name: "apple".to_string(),
                quantity: 1,
            }),
        )
        .unwrap();
        db.put(
            "fruit:banana",
            to_cbor(Fruit {
                name: "banana".to_string(),
                quantity: 2,
            }),
        )
        .unwrap();
        db.put(
            "fruit:kiwi",
            to_cbor(Fruit {
                name: "kiwi".to_string(),
                quantity: 3,
            }),
        )
        .unwrap();

        // Define some handler/worker task on the iterator. Here, we drop any apple and we change
        // the quantity of banana. We expects those to be persisted in the database.
        let handler = |iterator: IterBorrow<'_, '_, Option<Fruit>>| {
            for mut fruit in iterator {
                if fruit.borrow().as_ref().map(|f| f.name.as_str()) == Some("apple") {
                    *fruit.as_mut().borrow_mut() = None;
                } else if fruit.borrow().as_ref().map(|f| f.name.as_str()) == Some("banana") {
                    let fruit: &mut Option<Fruit> = fruit.as_mut().borrow_mut();
                    fruit.as_mut().unwrap().quantity = 42;
                }
            }
        };

        // Simulate a series of operation on the "fruit" table, deferring updates to a separate
        // 'handler' function.
        {
            let batch = db.transaction();
            let mut iterator: KeyValueIterator<'_, Fruit> = new(batch
                .prefix_iterator("fruit")
                .map(|item| item.unwrap_or_else(|e| panic!("unexpected database error: {e:?}"))));

            handler(Box::new(&mut iterator));

            // Apply updates to the database
            for (k, v) in iterator.into_iter_updates() {
                match v {
                    Some(v) => batch.put(k, to_cbor(v)),
                    None => batch.delete(k),
                }
                .unwrap();
            }

            // Ensure that the database is unchanged before we commit anything
            assert_eq!(
                db.get("fruit:apple").unwrap(),
                Some(to_cbor(Fruit {
                    name: "apple".to_string(),
                    quantity: 1,
                }))
            );
            assert_eq!(
                db.get("fruit:banana").unwrap(),
                Some(to_cbor(Fruit {
                    name: "banana".to_string(),
                    quantity: 2
                }))
            );
            assert_eq!(
                db.get("fruit:kiwi").unwrap(),
                Some(to_cbor(Fruit {
                    name: "kiwi".to_string(),
                    quantity: 3
                }))
            );

            batch.commit().unwrap();
        }

        // Inspect the database after we commit all updates.
        assert_eq!(db.get("fruit:apple").unwrap(), None);
        assert_eq!(
            db.get("fruit:banana").unwrap(),
            Some(to_cbor(Fruit {
                name: "banana".to_string(),
                quantity: 42
            }))
        );
        assert_eq!(
            db.get("fruit:kiwi").unwrap(),
            Some(to_cbor(Fruit {
                name: "kiwi".to_string(),
                quantity: 3
            }))
        );
    }
}

mod mutable_db_entry {
    use std::borrow::{Borrow, BorrowMut};

    /// A wrapper around an item that allows to perform a specific action when mutated. This is handy
    /// to provide a clean abstraction to layers upstream (e.g. Iterators) while performing
    /// implementation-specific actions (such as updating a persistent storage) when items are mutated.
    ///
    /// More specifically, the provided hook is called whenever the item is mutably borrowed; from
    /// which it may or may not be mutated. But, why would one mutably borrow the item if not to mutate
    /// it?
    pub struct MutableDBEntry<T, F>
    where
        F: FnMut(&T),
    {
        item: T,
        hook: F,
        borrowed: bool,
    }

    impl<T, F> MutableDBEntry<T, F>
    where
        F: FnMut(&T),
    {
        pub fn new(item: T, hook: F) -> Self {
            Self {
                item,
                hook,
                borrowed: false,
            }
        }
    }

    // Provide a read-only access, through an immutable borrow.
    impl<T, F> Borrow<T> for MutableDBEntry<T, F>
    where
        F: FnMut(&T),
    {
        fn borrow(&self) -> &T {
            &self.item
        }
    }

    // Provide a write access, through a mutable borrow.
    impl<T, F> BorrowMut<T> for MutableDBEntry<T, F>
    where
        F: FnMut(&T),
    {
        fn borrow_mut(&mut self) -> &mut T {
            self.borrowed = true;
            &mut self.item
        }
    }

    // Install a handler for the hook when the object is dropped from memory.
    impl<T, F> Drop for MutableDBEntry<T, F>
    where
        F: FnMut(&T),
    {
        fn drop(&mut self) {
            if self.borrowed {
                (self.hook)(&self.item);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn trigger_hook_on_mutation() {
            let mut xs = Vec::new();
            {
                let mut item = MutableDBEntry::new(42, |n| xs.push(*n));
                let item_ref: &mut usize = item.borrow_mut();
                *item_ref -= 28;
            }
            assert_eq!(xs, vec![14], "{xs:?}")
        }

        #[test]
        fn ignore_hook_on_simple_borrow() {
            let mut xs: Vec<usize> = Vec::new();
            {
                let item = MutableDBEntry::new(42, |n| xs.push(*n));
                let item_ref: &usize = item.borrow();
                assert_eq!(item_ref, &42);
            }
            assert!(xs.is_empty(), "{xs:?}")
        }
    }
}