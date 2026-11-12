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

//! Native key and CBOR readers with cardano-cli envelope compatibility and key-format tracing.
//!
//! A text envelope is a JSON file with a `type` field naming the wrapped entity and a
//! `cborHex` field holding the hex-encoded CBOR serialisation of that entity.

use std::{io, path::Path};

pub use amaru_kernel::utils::envelope::{HasEnvelope, ReadEnvelopeError, read_cbor_or_envelope};
use amaru_kernel::{cbor, utils::envelope};
use amaru_observability::trace;

/// Read an exact-size raw key or a cardano-cli envelope, tracing legacy format use.
/// File contents are zeroed on drop; the trace contains only the key kind and path.
pub fn read_key_file<const SIZE: usize, C, T, E>(
    path: &Path,
    key_kind: &str,
    ctx: &mut C,
    from_raw: impl FnOnce(&[u8; SIZE]) -> Result<T, E>,
) -> Result<T, E>
where
    T: for<'d> cbor::Decode<'d, C> + HasEnvelope,
    E: From<io::Error> + From<ReadEnvelopeError>,
{
    envelope::read_key_file(path, from_raw, || {
        let key = envelope::read_envelope(path, ctx)?;
        trace!(consensus::credentials::LEGACY_KEY_ENVELOPE, key_kind, path = path.to_string_lossy());
        Ok(key)
    })
}
