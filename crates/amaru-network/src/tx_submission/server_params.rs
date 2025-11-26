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

pub struct ServerParams {
    pub max_window: usize,  // how many tx ids we keep in “window”
    pub fetch_batch: usize, // how many txs we request per round
    pub blocking: Blocking, // should the client block when we request more tx ids that it cannot serve
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocking {
    Yes,
    No,
}

impl From<Blocking> for bool {
    fn from(value: Blocking) -> Self {
        match value {
            Blocking::Yes => true,
            Blocking::No => false,
        }
    }
}

impl ServerParams {
    pub fn new(max_window: usize, fetch_batch: usize, blocking: Blocking) -> Self {
        Self {
            max_window,
            fetch_batch,
            blocking,
        }
    }
}
