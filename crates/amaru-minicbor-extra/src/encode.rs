// Copyright 2026 PRAGMA
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

use minicbor::{self as cbor};

/// Encode a field with an optional value.
pub fn encode_optional<C, T, W>(
    e: &mut cbor::Encoder<W>,
    ctx: &mut C,
    key: u8,
    value: &Option<T>,
) -> Result<(), cbor::encode::Error<W::Error>>
where
    T: cbor::Encode<C>,
    W: cbor::encode::Write,
{
    if let Some(value) = value {
        e.u8(key)?.encode_with(value, ctx)?;
    }

    Ok(())
}
