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

/// This module is *temporary* and only necessary to re-implement a bug present in
/// the Cardano ledger in the protocol version 9; while it may sound weird, we have to
/// introduce this field in order to introduce data inconsistency issues to the ledger... on
/// purpose.
///
/// See also, Amaru's logbook entry from September.
///
/// It is temporary in the sense that, it is no longer required if bootstrapping from snapshots
/// that starts in protocol version 10 or later; and thus shall be dropped entirely when relevant.
use amaru_kernel::StakeCredential;
use std::collections::BTreeSet;

pub type Row = BTreeSet<StakeCredential>;
