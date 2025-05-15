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

use amaru_stores::rocksdb::common::as_key;
use iter_borrow::{new, IterBorrow, KeyValueIterator};
use pallas_codec::minicbor as cbor;
use rocksdb::OptimisticTransactionDB;
use tempfile::Builder;

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
    cbor::encode(&value, &mut buffer)
        .unwrap_or_else(|e| panic!("unable to encode value ({value:?}) to CBOR: {e:?}"));
    buffer
}

#[test]
fn db_iterator_mutate() {
    let db: OptimisticTransactionDB = OptimisticTransactionDB::open_default(
        Builder::new()
            .prefix("db_iterator_mutate-")
            .tempdir()
            .unwrap(),
    )
    .unwrap();

    let prefix = "fruit:".as_bytes();

    // Insert some values in a prefixed collection
    db.put(
        as_key(prefix, "apple"),
        to_cbor(Fruit {
            name: "apple".to_string(),
            quantity: 1,
        }),
    )
    .unwrap();
    db.put(
        as_key(prefix, "banana"),
        to_cbor(Fruit {
            name: "banana".to_string(),
            quantity: 2,
        }),
    )
    .unwrap();
    db.put(
        as_key(prefix, "kiwi"),
        to_cbor(Fruit {
            name: "kiwi".to_string(),
            quantity: 3,
        }),
    )
    .unwrap();

    // Define some handler/worker task on the iterator. Here, we drop any apple and we change
    // the quantity of banana. We expects those to be persisted in the database.
    let handler = |iterator: IterBorrow<'_, '_, String, Option<Fruit>>| {
        for (key, mut fruit) in iterator {
            if key == "apple" {
                *fruit.as_mut().borrow_mut() = None;
            } else if key == "banana" {
                let fruit: &mut Option<Fruit> = fruit.as_mut().borrow_mut();
                fruit.as_mut().unwrap().quantity = 42;
            } else {
                assert_eq!(fruit.borrow().as_ref().map(|f| f.quantity), Some(3));
            }
        }
    };

    // Simulate a series of operation on the "fruit" table, deferring updates to a separate
    // 'handler' function.
    {
        let batch = db.transaction();
        let mut iterator: KeyValueIterator<'_, 6, String, Fruit> = new(batch
            .prefix_iterator(prefix)
            .map(|item| item.unwrap_or_else(|e| panic!("unexpected database error: {e:?}"))));

        handler(Box::new(&mut iterator.as_iter_borrow()));

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
            db.get(as_key(prefix, "apple")).unwrap(),
            Some(to_cbor(Fruit {
                name: "apple".to_string(),
                quantity: 1,
            }))
        );
        assert_eq!(
            db.get(as_key(prefix, "banana")).unwrap(),
            Some(to_cbor(Fruit {
                name: "banana".to_string(),
                quantity: 2
            }))
        );
        assert_eq!(
            db.get(as_key(prefix, "kiwi")).unwrap(),
            Some(to_cbor(Fruit {
                name: "kiwi".to_string(),
                quantity: 3
            }))
        );

        batch.commit().unwrap();
    }

    // Inspect the database after we commit all updates.
    assert_eq!(db.get(as_key(prefix, "apple")).unwrap(), None);
    assert_eq!(
        db.get(as_key(prefix, "banana")).unwrap(),
        Some(to_cbor(Fruit {
            name: "banana".to_string(),
            quantity: 42
        }))
    );
    assert_eq!(
        db.get(as_key(prefix, "kiwi")).unwrap(),
        Some(to_cbor(Fruit {
            name: "kiwi".to_string(),
            quantity: 3
        }))
    );
}
