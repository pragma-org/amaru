// Copyright 2024 PRAGMA
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

use std::{ops::Deref, str::FromStr};

use amaru_kernel::{HeaderHash, Point};

pub(crate) mod dev;
#[cfg(feature = "mithril")]
pub(crate) mod mithril;
pub(crate) mod node;
pub(crate) mod shell_completions;
pub(crate) mod snapshot;

#[derive(Debug, Clone)]
pub(crate) struct PointOrHash(pub(crate) HeaderHash);
impl FromStr for PointOrHash {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<Point>().map(|p| p.hash()).or_else(|_| s.parse::<HeaderHash>().map_err(|e| e.to_string())).map(Self)
    }
}
impl Deref for PointOrHash {
    type Target = HeaderHash;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
