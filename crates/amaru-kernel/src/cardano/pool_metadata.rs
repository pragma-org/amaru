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

pub use pallas_primitives::conway::PoolMetadata;
use serde::ser::SerializeStruct;

#[derive(serde::Serialize)]
#[serde(transparent)]
pub struct AsJson<'a>(#[serde(serialize_with = "serialize")] pub &'a PoolMetadata);

pub fn serialize<S: serde::Serializer>(metadata: &PoolMetadata, serializer: S) -> Result<S::Ok, S::Error> {
    let mut s = serializer.serialize_struct("PoolMetadata", 2)?;
    // NOTE: keep fields in lexicographic order
    //
    // This instance is used for canonical ledger state comparisons.
    s.serialize_field("content_hash", &metadata.hash)?;
    s.serialize_field("url", &metadata.url)?;
    s.end()
}

pub fn as_option_ref(metadata: &Option<PoolMetadata>) -> Option<&PoolMetadata> {
    metadata.as_ref()
}

pub fn fmt(metadata: &Option<PoolMetadata>) -> String {
    match metadata {
        None => "ø".to_string(),
        Some(PoolMetadata { url, hash }) => format!("({}) {url}", &hex::encode(hash)[0..12]),
    }
}
