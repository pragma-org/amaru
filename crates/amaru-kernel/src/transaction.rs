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

use crate::{AuxiliaryData, Debug, TransactionBody, WitnessSet};

// TODO:
//
// This type is currently mostly unused; transactions in blocks have their constituents separated
// (i.e. seggregated witnesses). This type would however come in handy as soon as start accepting
// transactions from an external API.
//
// In which case, it'll become interesting to think about what public API we wanna expose. Exposing
// all fields an internals doesn't sound like a good idea and will likely break people's code
// (including ours) over time.
#[derive(Debug)]
pub struct Transaction {
    pub body: TransactionBody,
    pub witnesses: WitnessSet,
    pub is_expected_valid: bool,
    pub auxiliary_data: Option<AuxiliaryData>,
}
