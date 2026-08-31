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

use std::{collections::BTreeSet, env, str::FromStr};

use thiserror::Error;

/// Environment variable selecting which signals Amaru exports over OTLP.
pub const AMARU_OPEN_TELEMETRY_SIGNALS: &str = "AMARU_OPEN_TELEMETRY_SIGNALS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum OpenTelemetrySignal {
    Metrics,
    Traces,
    Logs,
}

impl OpenTelemetrySignal {
    const ALL: [Self; 3] = [Self::Metrics, Self::Traces, Self::Logs];
}

/// The OpenTelemetry signals enabled for OTLP export.
///
/// The environment representation is a comma-separated list containing one or
/// more of `metrics`, `traces`, and `logs`. When
/// [`AMARU_OPEN_TELEMETRY_SIGNALS`] is unset, all signals are enabled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenTelemetrySignals(BTreeSet<OpenTelemetrySignal>);

impl OpenTelemetrySignals {
    /// Read the selected signals from [`AMARU_OPEN_TELEMETRY_SIGNALS`].
    pub fn from_env() -> Result<Self, ParseOpenTelemetrySignalsError> {
        match env::var(AMARU_OPEN_TELEMETRY_SIGNALS) {
            Ok(value) => value.parse(),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err(ParseOpenTelemetrySignalsError::NotUnicode),
        }
    }

    pub fn metrics(&self) -> bool {
        self.0.contains(&OpenTelemetrySignal::Metrics)
    }

    pub fn traces(&self) -> bool {
        self.0.contains(&OpenTelemetrySignal::Traces)
    }

    pub fn logs(&self) -> bool {
        self.0.contains(&OpenTelemetrySignal::Logs)
    }
}

impl Default for OpenTelemetrySignals {
    fn default() -> Self {
        Self(OpenTelemetrySignal::ALL.into_iter().collect())
    }
}

impl FromStr for OpenTelemetrySignals {
    type Err = ParseOpenTelemetrySignalsError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.trim().is_empty() {
            return Err(ParseOpenTelemetrySignalsError::Empty);
        }

        let mut signals = BTreeSet::new();
        for signal in value.split(',').map(str::trim) {
            match signal.to_ascii_lowercase().as_str() {
                "metrics" => signals.insert(OpenTelemetrySignal::Metrics),
                "traces" => signals.insert(OpenTelemetrySignal::Traces),
                "logs" => signals.insert(OpenTelemetrySignal::Logs),
                _ => return Err(ParseOpenTelemetrySignalsError::Unknown(signal.to_string())),
            };
        }

        Ok(Self(signals))
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseOpenTelemetrySignalsError {
    #[error("{AMARU_OPEN_TELEMETRY_SIGNALS} must select at least one of: metrics, traces, logs")]
    Empty,
    #[error("unknown OpenTelemetry signal `{0}` in {AMARU_OPEN_TELEMETRY_SIGNALS}; expected metrics, traces, or logs")]
    Unknown(String),
    #[error("{AMARU_OPEN_TELEMETRY_SIGNALS} contains non-Unicode data")]
    NotUnicode,
}
