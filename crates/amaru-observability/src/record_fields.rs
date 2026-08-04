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

pub trait RecordFields {
    fn bool(&self, name: &str) -> Option<bool>;

    fn f64(&self, name: &str) -> Option<f64>;

    fn i64(&self, name: &str) -> Option<i64>;

    fn str(&self, name: &str) -> Option<&str>;

    fn u64(&self, name: &str) -> Option<u64>;

    fn u16(&self, name: &str) -> Option<u16> {
        self.u64(name).and_then(|value| u16::try_from(value).ok())
    }

    fn u32(&self, name: &str) -> Option<u32> {
        self.u64(name).and_then(|value| u32::try_from(value).ok())
    }

    fn usize(&self, name: &str) -> Option<usize> {
        self.u64(name).and_then(|value| usize::try_from(value).ok())
    }
}
