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

use std::{fs, path::Path};

use serde::Deserialize;
use thiserror::Error;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    r#type: String,
    cbor_hex: String,
}

/// Read a cardano-cli text envelope file, validate its type, and return the decoded CBOR bytes.
pub fn read_envelope(path: impl AsRef<Path>, expected_type: &'static str) -> Result<Vec<u8>, TextEnvelopeError> {
    let bytes = fs::read(path)?;
    let envelope: Envelope = serde_json::from_slice(&bytes)?;
    if envelope.r#type != expected_type {
        return Err(TextEnvelopeError::UnexpectedType { expected: expected_type, found: envelope.r#type });
    }
    Ok(hex::decode(envelope.cbor_hex)?)
}

/// The text envelope file could not be read or decoded.
#[derive(Debug, Error)]
pub enum TextEnvelopeError {
    #[error("unexpected envelope type: expected {expected}, found {found}")]
    UnexpectedType { expected: &'static str, found: String },
    #[error("failed to read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed envelope: {0}")]
    Json(#[from] serde_json::Error),
    #[error("hex is malformed: {0}")]
    Hex(#[from] hex::FromHexError),
}
