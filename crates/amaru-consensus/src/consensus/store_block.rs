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

use crate::{consensus::store_effects::StoreBlockEffect, span::adopt_current_span};
use amaru_kernel::block::ValidateBlockEvent;
use pure_stage::{Effects, StageRef, Void};
use tracing::Instrument;

type State = StageRef<ValidateBlockEvent, Void>;

pub async fn stage(
    downstream: State,
    msg: ValidateBlockEvent,
    eff: Effects<ValidateBlockEvent, State>,
) -> State {
    let span = adopt_current_span(&msg);
    async move {
        match &msg {
            ValidateBlockEvent::Validated { point, block, .. } => {
                if let Err(error) = eff
                    .external(StoreBlockEffect::new(block.clone(), point.clone()))
                    .await
                {
                    tracing::error!(%error, %point, "Failed to store block");
                    return eff.terminate().await;
                }
            }
            ValidateBlockEvent::Rollback { .. } => (),
        }
        eff.send(&downstream, msg).await;
        downstream
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ConsensusError,
        consensus::{
            store::{FakeStore, StoreError},
            store_effects::ResourceHeaderStore,
        },
    };
    use amaru_kernel::{Hash, Point};
    use pure_stage::{
        ExternalEffectAPI, StageGraph,
        simulation::{OverrideResult, SimulationBuilder},
    };
    use std::sync::Arc;
    use tokio::{runtime::Builder, sync::Mutex};
    use tracing::{Span, subscriber::DefaultGuard};
    use tracing_subscriber::util::SubscriberInitExt;

    #[test]
    fn test_forward() {
        let mut sim = SimulationBuilder::default();
        let stage = sim.stage("store_block", stage);
        let (output_ref, mut output) = sim.output("output", 10);
        let stage = sim.wire_up(stage, output_ref);

        let store = Arc::new(Mutex::new(FakeStore::default()));
        sim.resources().put::<ResourceHeaderStore>(store);

        let rt = Builder::new_current_thread().build().unwrap();
        let mut sim = sim.run(rt.handle().clone());

        let event = ValidateBlockEvent::Validated {
            point: Point::Specific(123, Hash::from([1; 32]).to_vec()),
            block: vec![0, 1, 2, 3],
            span: Span::current(),
        };

        sim.enqueue_msg(&stage, [event.clone()]);
        sim.run_until_blocked().assert_idle();
        assert_eq!(output.drain().collect::<Vec<_>>(), vec![event]);
    }

    #[test]
    fn test_forward_failure() {
        let log = TracingLog::new();

        let mut sim = SimulationBuilder::default();
        let stage = sim.stage("store_block", stage);
        let (output_ref, mut output) = sim.output("output", 10);
        let stage = sim.wire_up(stage, output_ref);

        let rt = Builder::new_current_thread().build().unwrap();
        let mut sim = sim.run(rt.handle().clone());

        let point = Point::Specific(123, Hash::from([1; 32]).to_vec());
        let event = ValidateBlockEvent::Validated {
            point: point.clone(),
            block: vec![0, 1, 2, 3],
            span: Span::current(),
        };

        sim.override_external_effect::<StoreBlockEffect>(1, move |_| {
            OverrideResult::Handled(Box::new(
                <StoreBlockEffect as ExternalEffectAPI>::Response::Err(
                    ConsensusError::StoreBlockFailed(
                        point.clone(),
                        StoreError::WriteError {
                            error: "booyah".to_string(),
                        },
                    ),
                ),
            ))
        });

        sim.enqueue_msg(&stage, [event.clone()]);
        sim.run_until_blocked().assert_idle();
        assert_eq!(output.drain().collect::<Vec<_>>(), vec![]);
        assert_eq!(sim.get_state(&stage), None);
        assert!(log.get_log().contains("Failed to store block body at 123.0101010101010101010101010101010101010101010101010101010101010101: WriteError: booyah"),
            "log must contain booyah:\n{}",
            log.get_log()
        );
    }

    #[test]
    fn test_rollback() {
        let mut sim = SimulationBuilder::default();
        let stage = sim.stage("store_block", stage);
        let (output_ref, mut output) = sim.output("output", 10);
        let stage = sim.wire_up(stage, output_ref);

        let rt = Builder::new_current_thread().build().unwrap();
        let mut sim = sim.run(rt.handle().clone());

        let expected_rollback_point = Point::Specific(100, Hash::from([2; 32]).to_vec());
        let event = ValidateBlockEvent::Rollback {
            rollback_point: expected_rollback_point.clone(),
            span: Span::current(),
        };

        sim.enqueue_msg(&stage, [event.clone()]);
        sim.run_until_blocked().assert_idle();
        assert_eq!(output.drain().collect::<Vec<_>>(), vec![event]);
    }

    struct TracingLog {
        _guard: DefaultGuard,
        buffer: BufferWriter,
    }

    impl TracingLog {
        fn new() -> Self {
            let buffer = BufferWriter::default();
            let _guard = tracing_subscriber::fmt()
                .with_ansi(false)
                .with_writer(buffer.clone())
                .set_default();
            Self { _guard, buffer }
        }

        fn get_log(&self) -> String {
            self.buffer.get_log()
        }
    }

    // A custom writer that captures logs into a String buffer
    #[derive(Clone, Default)]
    struct BufferWriter {
        pub buffer: Arc<parking_lot::Mutex<String>>,
    }

    impl BufferWriter {
        // Get the current contents of the buffer
        fn get_log(&self) -> String {
            self.buffer.lock().clone()
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for BufferWriter {
        type Writer = Self;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    // Implement std::io::Write to write log data to the buffer
    impl std::io::Write for BufferWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let s = String::from_utf8_lossy(buf);
            self.buffer.lock().push_str(&s);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
