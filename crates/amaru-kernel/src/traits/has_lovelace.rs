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

use crate::{Lovelace, MemoizedTransactionOutput, Value};

pub trait HasLovelace {
    fn lovelace(&self) -> Lovelace;
}

impl HasLovelace for Value {
    fn lovelace(&self) -> Lovelace {
        match self {
            Value::Coin(lovelace) => *lovelace,
            Value::Multiasset(lovelace, _) => *lovelace,
        }
    }
}

impl HasLovelace for MemoizedTransactionOutput {
    fn lovelace(&self) -> Lovelace {
        self.value.lovelace()
    }
}
