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
use std::ops::{Deref, DerefMut};

#[derive(Debug, Clone, Default, Eq, serde::Serialize, serde::Deserialize)]
pub struct IgnoreEq<T>(pub T);

impl<T> PartialEq for IgnoreEq<T> {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl<T> From<T> for IgnoreEq<T> {
    fn from(value: T) -> Self {
        IgnoreEq(value)
    }
}

impl<T> Deref for IgnoreEq<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T> DerefMut for IgnoreEq<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}
