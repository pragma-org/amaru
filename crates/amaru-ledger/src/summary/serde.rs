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

//! This module contains a variety of helpers used to produce serialised values for the various
//! summaries.

use std::collections::BTreeMap;

pub fn serialize_map<K, V: serde::ser::Serialize, S: serde::ser::SerializeStruct>(
    field: &'static str,
    s: &mut S,
    m: &BTreeMap<K, V>,
    serialize_key: impl Fn(&K) -> String,
) -> Result<(), S::Error> {
    let mut elems = m.iter().map(|(k, v)| (serialize_key(k), v)).collect::<Vec<_>>();
    elems.sort_by(|a, b| a.0.cmp(&b.0));
    s.serialize_field(field, &elems.into_iter().collect::<BTreeMap<String, &V>>())
}
