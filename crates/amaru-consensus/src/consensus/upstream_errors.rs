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

use crate::consensus::ValidationFailed;
use async_trait::async_trait;
use pure_stage::{Name, Referenceable, Stageable};

#[derive(Clone)]
pub struct UpstreamErrors;

impl UpstreamErrors {
    pub fn name() -> Name<ValidationFailed, ()> {
        Name::new("upstream_errors")
    }
}

#[async_trait]
impl Stageable<ValidationFailed, ()> for UpstreamErrors {
    fn initial_state(&self) {}

    async fn run(
        &self,
        _state: (),
        msg: ValidationFailed,
        eff: pure_stage::Effects<ValidationFailed>,
    ) -> () {
        // TODO: currently only validate_header errors, will need to grow into all error handling
        let ValidationFailed { peer, point, error } = msg;
        tracing::error!(%peer, %point, %error, "invalid header");
        // TODO: implement specific actions once we have an upstream network
        // termination here will tear down the entire stage graph
        eff.terminate().await
    }
}
