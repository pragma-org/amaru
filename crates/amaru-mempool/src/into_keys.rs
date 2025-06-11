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

use amaru_kernel::{TransactionInput, Tx};

/// An interface to obtain a set of keys for any given type, to be used as discriminants in a
/// mempool strategy.
pub trait IntoKeys {
    type Key;
    fn keys(&self) -> impl Iterator<Item = &Self::Key>;
}

impl IntoKeys for Tx {
    type Key = TransactionInput;

    fn keys(&self) -> impl Iterator<Item = &Self::Key> {
        self.transaction_body.inputs.iter()
    }
}
