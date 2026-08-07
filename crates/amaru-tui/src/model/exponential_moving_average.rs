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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExponentialMovingAverage {
    value: Option<f64>,
}

impl ExponentialMovingAverage {
    pub fn clear(&mut self) {
        self.value = None;
    }

    pub fn record(&mut self, sample: f64, smoothing: usize) {
        let alpha = 2.0 / (smoothing.max(1) as f64 + 1.0);
        self.value = Some(match self.value {
            Some(value) => alpha * sample + (1.0 - alpha) * value,
            None => sample,
        });
    }

    pub fn value(&self) -> Option<f64> {
        self.value
    }
}
