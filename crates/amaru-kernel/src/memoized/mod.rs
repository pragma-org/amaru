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

use crate::{Bytes, cbor, from_cbor};

pub mod datum;
pub use datum::*;

pub mod native_script;
pub use native_script::*;

pub mod plutus_data;
pub use plutus_data::*;

pub mod script;
pub use script::*;

pub mod transaction_output;
pub use transaction_output::*;

/// An implementation of 'TryFrom' from hex-encoded strings, for any type that can be decoded from
/// CBOR. Yields the original bytes and the deserialized value.
fn blanket_try_from_hex_bytes<T, I: for<'d> cbor::Decode<'d, ()>>(
    s: &str,
    new: impl Fn(Bytes, I) -> T,
) -> Result<T, String> {
    let original_bytes = Bytes::from(hex::decode(s.as_bytes()).map_err(|e| e.to_string())?);

    let value =
        from_cbor(&original_bytes).ok_or_else(|| "failed to decode from CBOR".to_string())?;

    Ok(new(original_bytes, value))
}
