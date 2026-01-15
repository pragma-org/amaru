#![expect(clippy::borrowed_box)]
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

use crate::SendData;
use cbor4ii::{
    core::{Value, utils::BufWriter},
    serde::{from_slice, to_writer},
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};
use std::{cell::RefCell, fmt};

/// Helper type to wrap futures/functions/etc. and thus avoid having to handroll
/// a `Debug` implementation for a type containing the wrapped value.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NoDebug<T>(pub T);
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

/// Diverging helper used in type contexts that expect `!`.
/// Panics intentionally; for non-resolving futures, use `std::future::pending()`.
#[cold]
#[inline(never)]
#[track_caller]
#[expect(clippy::panic)]
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

/// A trait to allow keeping collections of deserializer guards.
pub trait DeserializerGuard {}
pub type DeserializerGuards = Vec<Box<dyn DeserializerGuard>>;

enum Field {
    Typetag,
    Value,
    Ignored,
}
impl<'de> serde::de::Visitor<'de> for Field {
    type Value = Self;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "field identifier for SendDataValue")
    }
    fn visit_u64<E>(self, v: u64) -> Result<Self, E>
    where
        E: Error,
    {
        match v {
            0 => Ok(Field::Typetag),
            1 => Ok(Field::Value),
            _ => Ok(Field::Ignored),
        }
    }
    fn visit_str<E>(self, v: &str) -> Result<Self, E>
    where
        E: Error,
    {
        match v {
            "typetag" => Ok(Field::Typetag),
            "value" => Ok(Field::Value),
            _ => Ok(Field::Ignored),
        }
    }
    fn visit_bytes<E>(self, v: &[u8]) -> Result<Self, E>
    where
        E: Error,
    {
        match v {
            b"typetag" => Ok(Field::Typetag),
            b"value" => Ok(Field::Value),
            _ => Ok(Field::Ignored),
        }
    }
}
impl<'de> serde::de::Deserialize<'de> for Field {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(Field::Ignored)
    }
}

/// `#[serde(with = "pure_stage::serde::serialize_send_data")]` for serializing [`Box<dyn SendData>`](crate::SendData).
#[allow(clippy::disallowed_types)]
pub mod serialize_send_data {
    use super::*;
    use crate::SendData;
    use serde::de::Error;
    use std::{any::type_name, collections::HashMap, fmt, sync::Arc};

    pub fn serialize<S: Serializer>(
        data: &Box<dyn SendData>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        data.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<dyn SendData>, D::Error> {
        deserializer.deserialize_struct("SendData", &["typetag", "value"], Visitor)
    }

    /// TODO(network): needs proper docs
    pub fn register_data_deserializer<T: SendData + serde::de::DeserializeOwned>() -> DropGuard {
        let name = type_name::<T>();
        TYPES.with_borrow_mut(|types| {
            types.insert(
                name.to_string(),
                Arc::new(|deserializer| {
                    let value = T::deserialize(deserializer)?;
                    Ok(Box::new(value))
                }),
            );
        });
        DropGuard(name)
    }

    pub struct DropGuard(&'static str);
    impl DeserializerGuard for DropGuard {}
    impl DropGuard {
        pub fn boxed(self) -> Box<dyn DeserializerGuard> {
            Box::new(self)
        }
    }

    impl Drop for DropGuard {
        fn drop(&mut self) {
            TYPES.with_borrow_mut(|types| {
                types.remove(self.0);
            });
        }
    }

    type Deser = Arc<
        dyn for<'de> Fn(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<Box<dyn SendData>, erased_serde::Error>,
    >;
    thread_local! {
        static TYPES: RefCell<HashMap<String, Deser>> = RefCell::new(HashMap::new());
        static DESER: RefCell<Option<Deser>> = const { RefCell::new(None) };
    }

    struct Dessert(Box<dyn SendData>);
    impl<'de> serde::de::Deserialize<'de> for Dessert {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[allow(clippy::expect_used)]
            let deser = DESER
                .with(|deser| deser.borrow_mut().take())
                .expect("deser is set");
            let mut deserializer = <dyn erased_serde::Deserializer<'de>>::erase(deserializer);
            let value = deser(&mut deserializer).map_err(D::Error::custom)?;
            Ok(Dessert(value))
        }
    }

    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Box<dyn SendData>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "a SendData value")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let typetag = seq
                .next_element::<String>()?
                .ok_or_else(|| serde::de::Error::invalid_length(0, &"typetag & value"))?;
            if let Some(value) = TYPES.with(|types| {
                if let Some(factory) = types.borrow().get(&typetag) {
                    DESER.with_borrow_mut(|deser| deser.replace(factory.clone()));
                    let value = seq
                        .next_element::<Dessert>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?;
                    Ok(Some(value.0))
                } else {
                    Ok(None)
                }
            })? {
                return Ok(value);
            }
            let value = seq
                .next_element::<cbor4ii::core::Value>()?
                .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?;
            Ok(Box::new(SendDataValue { typetag, value }))
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let (Field::Typetag, typetag) = map
                .next_entry::<Field, String>()?
                .ok_or_else(|| serde::de::Error::missing_field("typetag"))?
            else {
                return Err(serde::de::Error::custom("typetag must be encoded first"));
            };
            if let Some(value) = TYPES.with(|types| {
                if let Some(factory) = types.borrow().get(&typetag) {
                    DESER.with_borrow_mut(|deser| deser.replace(factory.clone()));
                    let (Field::Value, value) = map
                        .next_entry::<Field, Dessert>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?
                    else {
                        return Err(serde::de::Error::custom(
                            "value must be encoded after typetag",
                        ));
                    };
                    Ok(Some(value.0))
                } else {
                    Ok(None)
                }
            })? {
                return Ok(value);
            }
            let (Field::Value, value) = map
                .next_entry::<Field, cbor4ii::core::Value>()?
                .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?
            else {
                return Err(serde::de::Error::custom(
                    "value must be encoded after typetag",
                ));
            };
            Ok(Box::new(SendDataValue { typetag, value }))
        }
    }
}

/// This is the wrapper representation of a [`SendData`](crate::SendData) value after being deserialized.
#[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SendDataValue {
    pub typetag: String,
    pub value: Value,
}

impl fmt::Display for SendDataValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        SendDataValue::format_cbor_value(&self.value, f)
    }
}

impl SendDataValue {
    /// Try to format the SendDataValue CBOR value as a human-readable string.
    fn format_cbor_value(value: &Value, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match value {
            Value::Text(s) => write!(f, "{}", s),
            Value::Null => write!(f, "null"),
            Value::Bool(v) => write!(f, "{v}"),
            Value::Integer(v) => write!(f, "{v}"),
            Value::Float(v) => write!(f, "{v}"),
            Value::Bytes(_) => write!(f, "<bytes>"),
            Value::Array(vs) => {
                match SendDataValue::array_as_bytes(vs) {
                    Some(bytes) => {
                        write!(f, "{hash}", hash = hex::encode(bytes.as_slice()))?;
                    }
                    None => {
                        write!(f, "[")?;
                        let mut first = true;
                        for v in vs {
                            if first {
                                first = false;
                            } else {
                                write!(f, ", ")?;
                            }
                            SendDataValue::format_cbor_value(v, f)?;
                        }
                        write!(f, "]")?;
                    }
                }
                Ok(())
            }
            Value::Map(vs) => {
                write!(f, "{{")?;
                let mut first = true;
                for (k, v) in vs {
                    if first {
                        first = false;
                    } else {
                        write!(f, ", ")?;
                    }
                    SendDataValue::format_cbor_value(k, f)?;
                    write!(f, ": ")?;
                    SendDataValue::format_cbor_value(v, f)?;
                }
                write!(f, "}}")?;
                Ok(())
            }
            Value::Tag(_, v) => SendDataValue::format_cbor_value(v.as_ref(), f),
            _ => Ok(()),
        }
    }

    fn array_as_bytes(vs: &Vec<Value>) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(vs.len());
        for v in vs {
            if let Value::Integer(n) = v {
                if *n < 0.into() || *n > (u8::MAX as u64).into() {
                    return None;
                } else {
                    out.push(*n as u8);
                }
            } else {
                return None;
            }
        }
        Some(out)
    }
}

impl SendDataValue {
    pub fn new<T: SendData>(value: &T) -> Self {
        Self::from(value as &dyn SendData)
    }

    /// Construct a boxed [`SendData`](crate::SendData) value from a concrete type.
    ///
    /// This is a convenience function that serializes the value to a vector of bytes and then
    /// deserializes it back into a boxed [`SendData`](crate::SendData) value. It is mostly
    /// useful in tests.
    pub fn boxed<T: SendData>(value: &T) -> Box<dyn SendData> {
        Box::new(Self::new(value))
    }

    pub fn from_json(tag: impl AsRef<str>, value: impl Serialize) -> Box<dyn SendData> {
        #[expect(clippy::expect_used)]
        Box::new(Self {
            typetag: tag.as_ref().to_string(),
            value: from_slice(&to_cbor(&value)).expect("round-trip serialization should not fail"),
        })
    }
}

impl From<&dyn SendData> for SendDataValue {
    #[expect(clippy::expect_used)]
    fn from(value: &dyn SendData) -> Self {
        let mut buf = cbor4ii::serde::Serializer::new(BufWriter::new(Vec::new()));
        value
            .serialize(&mut buf)
            .expect("serialization should not fail");
        let bytes = buf.into_inner().into_inner();
        cbor4ii::serde::from_slice::<SendDataValue>(&bytes)
            .expect("deserialization of serialized SendDataValue should not fail")
    }
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
#[allow(clippy::disallowed_types)]
pub mod serialize_external_effect {
    use super::*;
    use crate::{ExternalEffect, effect::UnknownExternalEffect};
    use std::{collections::HashMap, sync::Arc};

    pub fn serialize<S: Serializer>(
        data: &Box<dyn ExternalEffect>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        data.serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Box<dyn ExternalEffect>, D::Error> {
        deserializer.deserialize_struct("SendData", &["typetag", "value"], Visitor)
    }

    pub fn register_effect_deserializer<T: ExternalEffect + serde::de::DeserializeOwned>()
    -> DropGuard {
        let name = std::any::type_name::<T>();
        TYPES.with_borrow_mut(|types| {
            types.insert(
                name.to_string(),
                Arc::new(|deserializer| {
                    let value = T::deserialize(deserializer)?;
                    Ok(Box::new(value))
                }),
            );
        });
        DropGuard(name)
    }

    pub struct DropGuard(&'static str);
    impl DeserializerGuard for DropGuard {}
    impl DropGuard {
        pub fn boxed(self) -> Box<dyn DeserializerGuard> {
            Box::new(self)
        }
    }

    impl Drop for DropGuard {
        fn drop(&mut self) {
            TYPES.with_borrow_mut(|types| {
                types.remove(self.0);
            });
        }
    }

    type Deser = Arc<
        dyn for<'de> Fn(
            &mut dyn erased_serde::Deserializer<'de>,
        ) -> Result<Box<dyn ExternalEffect>, erased_serde::Error>,
    >;
    thread_local! {
        static TYPES: RefCell<HashMap<String, Deser>> = RefCell::new(HashMap::new());
        static DESER: RefCell<Option<Deser>> = const { RefCell::new(None) };
    }

    struct Dessert(Box<dyn ExternalEffect>);
    impl<'de> serde::de::Deserialize<'de> for Dessert {
        fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
        where
            D: Deserializer<'de>,
        {
            #[expect(clippy::expect_used)]
            let deser = DESER
                .with(|deser| deser.borrow_mut().take())
                .expect("deser is set");
            let mut deserializer = <dyn erased_serde::Deserializer<'de>>::erase(deserializer);
            let value = deser(&mut deserializer).map_err(D::Error::custom)?;
            Ok(Dessert(value))
        }
    }

    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = Box<dyn ExternalEffect>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "a ExternalEffect value")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let typetag = seq
                .next_element::<String>()?
                .ok_or_else(|| serde::de::Error::invalid_length(0, &"typetag & value"))?;
            if let Some(value) = TYPES.with(|types| {
                if let Some(factory) = types.borrow().get(&typetag) {
                    DESER.with_borrow_mut(|deser| deser.replace(factory.clone()));
                    let value = seq
                        .next_element::<Dessert>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?;
                    Ok(Some(value.0))
                } else {
                    Ok(None)
                }
            })? {
                return Ok(value);
            }
            let value = seq
                .next_element::<cbor4ii::core::Value>()?
                .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?;
            Ok(Box::new(UnknownExternalEffect::new(SendDataValue {
                typetag,
                value,
            })))
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let (Field::Typetag, typetag) = map
                .next_entry::<Field, String>()?
                .ok_or_else(|| serde::de::Error::missing_field("typetag"))?
            else {
                return Err(serde::de::Error::custom("typetag must be encoded first"));
            };
            if let Some(value) = TYPES.with(|types| {
                if let Some(factory) = types.borrow().get(&typetag) {
                    DESER.with_borrow_mut(|deser| deser.replace(factory.clone()));
                    let (Field::Value, value) = map
                        .next_entry::<Field, Dessert>()?
                        .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?
                    else {
                        return Err(serde::de::Error::custom(
                            "value must be encoded after typetag",
                        ));
                    };
                    Ok(Some(value.0))
                } else {
                    Ok(None)
                }
            })? {
                return Ok(value);
            }
            let (Field::Value, value) = map
                .next_entry::<Field, cbor4ii::core::Value>()?
                .ok_or_else(|| serde::de::Error::invalid_length(1, &"value"))?
            else {
                return Err(serde::de::Error::custom(
                    "value must be encoded after typetag",
                ));
            };
            Ok(Box::new(UnknownExternalEffect::new(SendDataValue {
                typetag,
                value,
            })))
        }
    }
}

/// Serialize a value to a vector of bytes, using a thread-local buffer to
/// optimize allocations.
pub fn to_cbor<T: serde::Serialize>(value: &T) -> Vec<u8> {
    thread_local! {
        static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    }
    BUFFER.with_borrow_mut(|buffer| {
        #[expect(clippy::expect_used)]
        to_writer(&mut *buffer, value).expect("serialization should not fail");
        let ret = Vec::from(buffer.as_slice());
        buffer.clear();
        ret
    })
}

/// Deserialize a value from a vector of bytes
pub fn from_cbor<'a, T: serde::Deserialize<'a>>(value: &'a Vec<u8>) -> anyhow::Result<T> {
    Ok(cbor4ii::serde::from_slice::<T>(value.as_slice())?)
}
