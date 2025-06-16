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

/// Generic wrapper around sending logic.
///
/// Just adds additional error messages in order to better locate the sources
/// of errors when a gasket stage fails.
#[macro_export]
macro_rules! send {
    ($port: expr, $msg: expr) => {
    $port.send($msg.into()).await.map_err(|e| {
        tracing::error!(error=%e, "failed to send event");
        gasket::framework::WorkerError::Send
      })
    }
}

/// Generic scheduling function for the common case where we immediately dispatch
/// received message to be processed.
///
/// Mostly useful to add more context to errors and reduce boilerplate.
#[macro_export]
macro_rules! schedule {
    ($port: expr) => {
    $port.recv()
        .await
        .map_err(|e| {
            tracing::error!(error=%e, "error receiving message");
            gasket::framework::WorkerError::Recv
        })
        .map(|unit| gasket::framework::WorkSchedule::Unit(unit.payload))
  }
}
