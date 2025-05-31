use futures_util::StreamExt;
use pure_stage::{tokio::TokioBuilder, StageGraph};
use std::time::Duration;
use tokio::time::timeout;

#[test]
fn basic() {
    tracing_subscriber::fmt()
        .with_test_writer()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let rt = tokio::runtime::Runtime::new().unwrap();

    let mut graph = TokioBuilder::default();
    let double = graph.stage("double", async |to, msg: u32, eff| {
        eff.send(&to, msg * 2).await;
        Ok(to)
    });
    let (out_ref, mut out_rx) = graph.output("output", 10);
    let double = graph.wire_up(double, out_ref);

    let send_double = graph.sender(&double);

    let handle = rt.handle().clone();
    rt.block_on(async move {
        // running needs `tokio::spawn` to work
        let graph = graph.run(handle);

        for i in 0..10 {
            tracing::info!("sending {}", i);
            timeout(Duration::from_secs(1), send_double.send(i))
                .await
                .unwrap()
                .unwrap();
        }

        for i in 0..10 {
            tracing::info!("receiving {}", i);
            assert_eq!(
                timeout(Duration::from_secs(1), out_rx.next())
                    .await
                    .unwrap(),
                Some(i * 2)
            );
        }

        graph.abort();
    });
}
