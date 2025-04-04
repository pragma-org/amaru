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

use amaru_kernel::Point;
use tracing::Span;

pub type RawBlock = Vec<u8>;

#[derive(Clone, Debug)]
pub enum ValidateBlockEvent {
    Validated(Point, RawBlock, Span),
    Rollback(Point),
}

#[derive(Clone, Debug)]
pub enum BlockValidationResult {
    BlockValidated(Point, Span),
    BlockForwardStorageFailed(Point, Span),
    InvalidRollbackPoint(Point),
    RolledBackTo(Point),
}

pub mod context;
pub mod rules;
pub mod state;
pub mod store;
pub mod summary;

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        io::Write,
        sync::{Arc, Mutex},
    };

    use amaru_kernel::{
        Bytes, Hash, PostAlonzoTransactionOutput, TransactionInput, TransactionOutput, Value,
    };

    use tracing::Dispatch;
    use tracing_subscriber::{
        fmt::{self, format::FmtSpan},
        layer::SubscriberExt,
    };

    // -----------------------------------------------------------------------------
    // Testing Macros
    // -----------------------------------------------------------------------------
    macro_rules! include_transaction_body {
        ($test_directory:literal, $hash:literal) => {
            cbor::decode::<KeepRaw<'_, MintedTransactionBody<'_>>>(include_bytes!(concat!(
                $test_directory,
                "/data/transactions/preprod/",
                $hash,
                "/tx.cbor"
            )))
            .unwrap()
        };
        ($test_directory:literal, $hash:literal, $test_variant:literal) => {
            cbor::decode::<KeepRaw<'_, MintedTransactionBody<'_>>>(include_bytes!(concat!(
                $test_directory,
                "/data/transactions/preprod/",
                $hash,
                "/",
                $test_variant,
                "/tx.cbor"
            )))
            .unwrap()
        };
    }

    macro_rules! include_witness_set {
        ($test_directory:literal, $hash:literal, $test_variant:literal) => {
            from_cbor::<WitnessSet>(include_bytes!(concat!(
                $test_directory,
                "/data/transactions/preprod/",
                $hash,
                "/",
                $test_variant,
                "/witness.cbor"
            )))
            .unwrap()
        };
        ($test_directory:literal, $hash:literal) => {
            from_cbor::<WitnessSet>(include_bytes!(concat!(
                $test_directory,
                "/data/transactions/preprod/",
                $hash,
                "/witness.cbor"
            )))
            .unwrap()
        };
    }

    macro_rules! include_expected_traces {
        ($path:literal, $hash:literal) => {
            include_str!(concat!(
                $path,
                "/data/transactions/preprod/",
                $hash,
                "/expected.traces"
            ))
        };
    }

    pub(crate) use include_expected_traces;
    pub(crate) use include_transaction_body;
    pub(crate) use include_witness_set;

    // -----------------------------------------------------------------------------
    // Tracing for Tests
    // -----------------------------------------------------------------------------

    #[derive(Clone)]
    pub(crate) struct TestingTraceCollector {
        pub lines: Arc<Mutex<Vec<String>>>,
    }

    impl TestingTraceCollector {
        pub fn new() -> Self {
            TestingTraceCollector {
                lines: Arc::new(Mutex::new(Vec::new())),
            }
        }
        pub fn clear(&mut self) {
            self.lines.lock().unwrap().clear();
        }
    }

    impl Write for TestingTraceCollector {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if let Ok(s) = std::str::from_utf8(buf) {
                for line in s.lines() {
                    if !line.is_empty() {
                        self.lines.lock().unwrap().push(line.to_string());
                    }
                }
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    pub(crate) fn with_tracing<F, R>(test_fn: F) -> R
    where
        F: FnOnce(&TestingTraceCollector) -> R,
    {
        let mut collector = TestingTraceCollector::new();
        collector.clear();
        let collector_clone = collector.clone();
        let format_layer = fmt::layer()
            .with_writer(move || collector_clone.clone())
            .json()
            .with_span_events(FmtSpan::ENTER)
            .without_time()
            .with_level(false)
            .with_target(false)
            .with_thread_ids(false)
            .with_thread_names(false)
            .with_ansi(false)
            .compact();

        let subscriber = tracing_subscriber::registry().with(format_layer);

        let dispatch = Dispatch::new(subscriber);
        // explicit variable here to ensure it survives the lifetime of the test function
        let _guard = tracing::dispatcher::set_default(&dispatch);
        test_fn(&collector)
    }

    // -----------------------------------------------------------------------------
    // Test Helpers
    // -----------------------------------------------------------------------------

    #[derive(Debug)]
    // This is just to provide additional context when a test fails due to invalid tracing
    // Not actually used in code (except for Debug), so we have to allow dead_code
    #[allow(dead_code)]
    pub(crate) enum InvalidTrace {
        TraceLengthMismatch {
            expected: usize,
            actual: usize,
        },
        InvalidTrace {
            expected: String,
            actual: String,
            index: usize,
        },
    }

    pub(crate) fn verify_traces(
        collected_traces: Vec<String>,
        expected_traces: &str,
    ) -> Result<(), InvalidTrace> {
        let expected_traces: Vec<String> = expected_traces
            .lines()
            .filter_map(|line| {
                if line.trim().is_empty() {
                    None
                } else {
                    Some(line.to_string())
                }
            })
            .collect();

        if collected_traces.len() != expected_traces.len() {
            return Err(InvalidTrace::TraceLengthMismatch {
                expected: expected_traces.len(),
                actual: collected_traces.len(),
            });
        }

        for (index, (actual, expected)) in collected_traces
            .into_iter()
            .zip(expected_traces.into_iter())
            .enumerate()
        {
            if actual != expected {
                return Err(InvalidTrace::InvalidTrace {
                    expected,
                    actual,
                    index,
                });
            }
        }

        Ok(())
    }

    pub(crate) fn fake_input(transaction_id: &str, index: u64) -> TransactionInput {
        TransactionInput {
            transaction_id: Hash::from(hex::decode(transaction_id).unwrap().as_slice()),
            index,
        }
    }

    pub(crate) fn fake_output(address: &str) -> TransactionOutput {
        TransactionOutput::PostAlonzo(PostAlonzoTransactionOutput {
            address: Bytes::from(hex::decode(address).expect("Invalid hex address")),
            value: Value::Coin(0),
            datum_option: None,
            script_ref: None,
        })
    }
}
