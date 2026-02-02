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

pub mod ignore_eq;
pub mod key_value_pairs;
pub mod legacy;
pub mod non_empty_bytes;
pub mod non_empty_key_value_pairs;
pub use non_empty_key_value_pairs::*;
pub mod non_empty_set;
pub mod non_empty_vec;
// TODO: remove 'Nullable', eventually
//
// This type only exists for the sake of preserving CBOR structure at the Rust-level. It is,
// however, usually unnecessary and only make code harder to deal with down the line. It should be
// fully replaced with options.
pub mod nullable;
pub mod set;
pub mod strict_maybe;
