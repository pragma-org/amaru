#![allow(dead_code, clippy::borrowed_box)]
// Copyright 2025 PRAGMA
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

//! This module contains some serialization and deserialization code for the Pure Stage library.

use cbor4ii::serde::to_writer;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;

/// Helper type to wrap futures/functions/etc. and thus avoid having to handroll
/// a `Debug` implementation for a type containing the wrapped value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct NoDebug<T>(T);
impl<T> NoDebug<T> {
    pub fn new(t: T) -> Self {
        Self(t)
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}
impl<T> std::ops::Deref for NoDebug<T> {
    type Target = T;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl<T> std::ops::DerefMut for NoDebug<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
impl<T> std::fmt::Debug for NoDebug<T> {
    fn fmt(&self, _f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

/// A type that is not inhabited.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Void {}

/// Diverging helper used in type contexts that expect `!`.
/// Panics intentionally; for non-resolving futures, use `std::future::pending()`.
#[cold]
#[inline(never)]
#[track_caller]
#[allow(clippy::panic)]
pub fn never() -> ! {
    panic!("unreachable: never")
}

/// `#[serde(with = "pure_stage::serde::serialize_error")]` for serializing [`anyhow::Error`].
pub mod serialize_error {
    use super::*;

    pub fn serialize<S: Serializer>(
        error: &anyhow::Error,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(error.to_string().as_str())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<anyhow::Error, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(anyhow::Error::msg(s))
    }
}

/// `#[serde(with = "pure_stage::serde::serialize_send_data")]` for serializing [`Box<dyn SendData>`](crate::SendData).
pub mod serialize_send_data {
    use super::*;
    use crate::SendData;

    pub fn serialize<S: Serializer>(
        data: &Box<dyn SendData>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        data.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<dyn SendData>, D::Error> {
        let s = SendDataValue::deserialize(deserializer)?;
        Ok(Box::new(s) as Box<dyn SendData>)
    }
}

/// This is the wrapper representation of a [`SendData`](crate::SendData) value when serialized and deserialized.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendDataValue {
    pub typetag: String,
    pub value: cbor4ii::core::Value,
}

#[cfg(test)]
mod test_send_data {
    use super::*;
    use crate::SendData;
    use cbor4ii::serde::from_slice;

    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestTuple(String, u32);
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    struct TestStruct {
        a: String,
        b: u32,
    }
    #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
    enum TestEnum {
        A(String),
        B(u32),
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    struct Container(#[serde(with = "serialize_send_data")] Box<dyn SendData>);

    #[test]
    fn test_tuple() {
        let value = TestTuple("hello".to_string(), 42);
        let c1 = Container(Box::new(value.clone()));
        let bytes = to_cbor(&c1);

        let c2: Container = from_slice(&bytes).unwrap();
        let cv = c2.0.cast_deserialize::<TestTuple>().unwrap();
        assert_eq!(cv, value);

        let c3: Container = from_slice(&bytes).unwrap();
        let cv = cv.deserialize_value(&*c3.0).unwrap();
        assert_eq!(&*cv, &*c1.0);
    }

    #[test]
    fn test_struct() {
        let value = TestStruct {
            a: "hello".to_string(),
            b: 42,
        };
        let c1 = Container(Box::new(value.clone()));
        let bytes = to_cbor(&c1);

        let c2: Container = from_slice(&bytes).unwrap();
        let cv = c2.0.cast_deserialize::<TestStruct>().unwrap();
        assert_eq!(cv, value);

        let c3: Container = from_slice(&bytes).unwrap();
        let cv = cv.deserialize_value(&*c3.0).unwrap();
        assert_eq!(&*cv, &*c1.0);
    }

    #[test]
    fn test_enum_a() {
        let value = TestEnum::A("hello".to_string());
        let c1 = Container(Box::new(value.clone()));
        let bytes = to_cbor(&c1);

        let c2: Container = from_slice(&bytes).unwrap();
        let cv = c2.0.cast_deserialize::<TestEnum>().unwrap();
        assert_eq!(cv, value);

        let c3: Container = from_slice(&bytes).unwrap();
        let cv = cv.deserialize_value(&*c3.0).unwrap();
        assert_eq!(&*cv, &*c1.0);
    }

    #[test]
    fn test_enum_b() {
        let value = TestEnum::B(42);
        let c1 = Container(Box::new(value.clone()));
        let bytes = to_cbor(&c1);

        let c2: Container = from_slice(&bytes).unwrap();
        let cv = c2.0.cast_deserialize::<TestEnum>().unwrap();
        assert_eq!(cv, value);

        let c3: Container = from_slice(&bytes).unwrap();
        let cv = cv.deserialize_value(&*c3.0).unwrap();
        assert_eq!(&*cv, &*c1.0);
    }
}

/// `#[serde(with = "pure_stage::serde::serialize_external_effect")]` for serializing [`Box<dyn ExternalEffect>`](crate::ExternalEffect).
pub mod serialize_external_effect {
    use super::*;
    use crate::{ExternalEffect, effect::UnknownExternalEffect};

    pub fn serialize<S: Serializer>(
        data: &Box<dyn ExternalEffect>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        data.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<dyn ExternalEffect>, D::Error> {
        let s = SendDataValue::deserialize(deserializer)?;
        Ok(Box::new(UnknownExternalEffect::new(s)))
    }
}

/// Serialize a value to a vector of bytes, using a thread-local buffer to
/// optimize allocations.
pub fn to_cbor<T: serde::Serialize>(value: &T) -> Vec<u8> {
    thread_local! {
        static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    BUFFER.with_borrow_mut(|buffer| {
        #[allow(clippy::expect_used)]
        to_writer(&mut *buffer, value).expect("serialization should not fail");
        let ret = Vec::from(buffer.as_slice());
        buffer.clear();
        ret
    })
}
