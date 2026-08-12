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

use crate::{AuxiliaryData, TransactionBody, TransactionId, WitnessSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct TransactionRef<'a> {
    pub body: &'a TransactionBody,
    pub witnesses: &'a WitnessSet,
    pub is_expected_valid: bool,
    pub auxiliary_data: Option<&'a AuxiliaryData>,
}

impl<'a> TransactionRef<'a> {
    pub fn tx_id(&self) -> TransactionId {
        TransactionId::new(self.body.id())
    }
}
