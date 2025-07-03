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

use crate::cbor;

#[derive(Debug)]
pub enum StrictMaybe<T> {
    Nothing,
    Just(T),
}

impl<'b, C, T: cbor::decode::Decode<'b, C>> cbor::decode::Decode<'b, C> for StrictMaybe<T> {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        if len != Some(0) {
            let t = d.decode_with(ctx)?;
            Ok(StrictMaybe::Just(t))
        } else {
            Ok(StrictMaybe::Nothing)
        }
    }
}

impl<T> From<StrictMaybe<T>> for Option<T> {
    fn from(value: StrictMaybe<T>) -> Option<T> {
        match value {
            StrictMaybe::Nothing => None,
            StrictMaybe::Just(t) => Some(t),
        }
    }
}
