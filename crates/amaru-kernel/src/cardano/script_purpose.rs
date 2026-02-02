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

pub use pallas_primitives::conway::RedeemerTag as ScriptPurpose;

// TODO: replace with IntoString instance
pub fn script_purpose_to_string(purpose: &ScriptPurpose) -> String {
    match purpose {
        ScriptPurpose::Spend => "Spend".to_string(),
        ScriptPurpose::Mint => "Mint".to_string(),
        ScriptPurpose::Cert => "Cert".to_string(),
        ScriptPurpose::Reward => "Reward".to_string(),
        ScriptPurpose::Vote => "Vote".to_string(),
        ScriptPurpose::Propose => "Propose".to_string(),
    }
}
