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

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupContext {
    pub process: ProcessInfo,
    pub protocol_version: String,
    pub epoch_length: u64,
    pub active_slot_coeff_inverse: u64,
    pub system_start_millis: u64,
    pub trusted_peers: BTreeSet<String>,
    pub runtime_sections: Vec<ConfigSection>,
    pub global_sections: Vec<ConfigSection>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub network: String,
    pub software_version: String,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigSection {
    pub title: &'static str,
    pub entries: Vec<ConfigEntry>,
}

impl ConfigSection {
    pub fn new(title: &'static str, entries: Vec<ConfigEntry>) -> Self {
        Self { title, entries }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub label: &'static str,
    pub option: Option<&'static str>,
    pub env_var: Option<&'static str>,
    pub value: String,
}

impl ConfigEntry {
    pub fn new(
        label: &'static str,
        option: Option<&'static str>,
        env_var: Option<&'static str>,
        value: impl Into<String>,
    ) -> Self {
        Self { label, option, env_var, value: value.into() }
    }
}
