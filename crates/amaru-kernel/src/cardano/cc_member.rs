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

use crate::{CommitteeAuthorization, Epoch, StakeCredential};

#[derive(Debug, Clone, PartialEq)]
pub struct CCMember {
    /// The member's authorization: `None` when elected but no hot key declared and not
    /// resigned, otherwise its declared hot credential or its resignation.
    pub authorization: Option<CommitteeAuthorization>,
    /// The term expiry; `None` once the member is inactive.
    pub valid_until: Option<Epoch>,
}

impl CCMember {
    /// The authorized hot credential, if the member has declared one and has not resigned.
    pub fn hot_key(&self) -> Option<&StakeCredential> {
        match &self.authorization {
            Some(CommitteeAuthorization::HotCredential(hot)) => Some(hot),
            _ => None,
        }
    }

    /// Whether the member has resigned its cold credential.
    pub fn has_resigned(&self) -> bool {
        matches!(self.authorization, Some(CommitteeAuthorization::Resigned(_)))
    }
}
