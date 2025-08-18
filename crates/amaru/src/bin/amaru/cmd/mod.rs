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

use amaru_kernel::network::NetworkName;

pub(crate) mod bootstrap;
pub(crate) mod daemon;
pub(crate) mod import_headers;
pub(crate) mod import_ledger_state;
pub(crate) mod import_nonces;

pub(crate) const DEFAULT_NETWORK: NetworkName = NetworkName::Preprod;

/// Default address to listen on for incoming connections.
pub(crate) const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:3000";
