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

use serde::{Deserialize, Serialize};
use std::fmt::Display;

#[derive(Serialize, Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct Envelope<T> {
    pub src: String,
    pub dest: String,
    pub body: T,
}

impl<T: Display> Display for Envelope<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} -> {}: {}", self.src, self.dest, self.body)
    }
}

impl<T> Envelope<T> {
    pub fn new(src: String, dest: String, body: T) -> Self {
        Self { src, dest, body }
    }

    pub fn is_client_message(&self) -> bool {
        self.src.starts_with("c")
    }
}
