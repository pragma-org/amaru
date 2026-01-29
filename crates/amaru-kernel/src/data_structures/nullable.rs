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

pub use pallas_codec::utils::Nullable;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    pub fn any_nullable<T: std::fmt::Debug + Clone>(
        any_inner: impl Strategy<Value = T>,
    ) -> impl Strategy<Value = Nullable<T>> {
        prop_oneof![
            Just(Nullable::Undefined),
            Just(Nullable::Null),
            any_inner.prop_map(Nullable::Some)
        ]
    }
}
