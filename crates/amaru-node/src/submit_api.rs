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

use std::net::SocketAddr;

use amaru_observability::{info, warn};
use amaru_ouroboros::TxRejectReason;
use anyhow::Context;
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::mempool::{MempoolSubmitError, MempoolSubmitter, MempoolUnavailableReason};

type SubmitApiState = MempoolSubmitter;

/// Start HTTP submission using the node's typed service and lifecycle admission.
pub async fn start(
    addr: SocketAddr,
    submitter: MempoolSubmitter,
    shutdown: CancellationToken,
) -> anyhow::Result<(JoinHandle<()>, SocketAddr)> {
    let app = Router::new().route("/api/submit/tx", post(submit_tx)).with_state(submitter);

    let listener =
        TcpListener::bind(addr).await.with_context(|| format!("failed to bind submit API address at {addr}"))?;
    let local_addr = listener.local_addr().context("failed to get local address")?;

    info!(node::submit_api::STARTED, local_addr = local_addr.to_string());

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).with_graceful_shutdown(shutdown.cancelled_owned()).await {
            warn!(node::submit_api::STOPPED, error = err.to_string());
        }
    });

    Ok((handle, local_addr))
}

/// Handle incoming transaction submission requests.
/// The request body is expected to be a CBOR-encoded.
async fn submit_tx(State(submitter): State<SubmitApiState>, headers: HeaderMap, body: Bytes) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();

    if !content_type.eq_ignore_ascii_case("application/cbor") {
        return text_response(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Content-Type must be application/cbor");
    }

    match submitter.submit(&body).await {
        Ok(accepted) => json_response(StatusCode::ACCEPTED, accepted.transaction_id.to_string()),
        Err(error @ MempoolSubmitError::InputTooLarge { .. }) => {
            text_response(StatusCode::PAYLOAD_TOO_LARGE, error.to_string())
        }
        Err(MempoolSubmitError::InvalidCbor { reason }) => {
            text_response(StatusCode::BAD_REQUEST, format!("Invalid CBOR transaction: {reason}"))
        }
        Err(MempoolSubmitError::Rejected { reason, .. }) => text_response(
            match reason {
                TxRejectReason::MempoolFull => StatusCode::SERVICE_UNAVAILABLE,
                TxRejectReason::Duplicate => StatusCode::CONFLICT,
                TxRejectReason::Invalid(_) => StatusCode::BAD_REQUEST,
            },
            reason.to_string(),
        ),
        Err(MempoolSubmitError::NotAdmitted { .. }) => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "mempool deadline reached before queue admission")
        }
        Err(MempoolSubmitError::Timeout { .. }) => text_response(StatusCode::SERVICE_UNAVAILABLE, "mempool timed out"),
        Err(MempoolSubmitError::Unavailable { reason: MempoolUnavailableReason::SendFailed, .. }) => {
            warn!(node::submit_api::MEMPOOL_UNREACHABLE, reason = "send_failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool unavailable")
        }
        Err(MempoolSubmitError::Unavailable { reason: MempoolUnavailableReason::ResponseDropped, .. }) => {
            warn!(node::submit_api::MEMPOOL_UNREACHABLE, reason = "response_dropped");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool unavailable")
        }
        Err(MempoolSubmitError::Unavailable {
            reason: MempoolUnavailableReason::ResponseDeserializeFailed, ..
        }) => {
            warn!(node::submit_api::MEMPOOL_UNREACHABLE, reason = "deserialize_failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool returned an invalid response")
        }
        Err(MempoolSubmitError::Closing { .. } | MempoolSubmitError::Stopped { .. }) => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "mempool unavailable")
        }
    }
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, Json(body)).into_response()
}

fn text_response(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], body.into()).into_response()
}

#[cfg(test)]
mod tests {
    use std::{net::SocketAddr, sync::Arc};

    use amaru_consensus::{
        effects::{ResourceBlockValidation, ResourceEraHistory, ResourceTxValidation},
        stages::mempool::MempoolStageState,
    };
    use amaru_kernel::{RawBlock, Transaction, cbor::WithOriginalBytes, to_cbor};
    use amaru_mempool::{InMemoryMempool, MempoolConfig};
    use amaru_ouroboros::ResourceMempool;
    use amaru_ouroboros_traits::{
        MockBlockValidator, MockCanValidateTxs, TransactionValidationError, TxSubmissionMempool,
    };
    use amaru_protocols::store_effects::ResourceParameters;
    use amaru_pure_stage::{
        StageGraph, StageGraphRunning,
        tokio::{TokioBuilder, TokioRunning},
    };
    use axum::{
        body::{Bytes, to_bytes},
        extract::State,
        http::HeaderMap,
    };
    use reqwest::{Response, header::CONTENT_TYPE};
    use tokio::{runtime::Handle, task::JoinHandle};
    use tokio_util::sync::CancellationToken;

    use super::start;
    use crate::{
        mempool::{MempoolRuntime, NodeRunId},
        stages::config::Config,
        tests::test_data::create_transaction,
    };

    type TestMempool = Arc<InMemoryMempool<WithOriginalBytes<Transaction>>>;

    struct TestSubmitApi {
        address: SocketAddr,
        shutdown: CancellationToken,
        runtime: Arc<MempoolRuntime>,
        running: TokioRunning,
        server: JoinHandle<()>,
    }

    impl Drop for TestSubmitApi {
        fn drop(&mut self) {
            self.runtime.close();
            self.shutdown.cancel();
            self.running.request_abort();
            self.server.abort();
        }
    }

    #[tokio::test]
    async fn test_successful_submission() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let tx = create_transaction(0);
        let expected_tx_id = tx.tx_id();
        let body = amaru_kernel::to_cbor(&tx);

        let resp = submit_tx(addr, body).await?;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");

        let text = resp.text().await?;
        assert_eq!(text, format!("\"{expected_tx_id}\""));
        Ok(())
    }

    #[tokio::test]
    async fn test_submission_of_with_transaction_extracted_from_existing_block() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let body = serialized_transaction()?;
        let expected_tx: Transaction = minicbor::decode(&body)?;
        let expected_tx_id = expected_tx.tx_id();
        assert_eq!(expected_tx_id.to_string(), TX_ID);

        let resp = submit_tx(addr, body).await?;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");

        let text = resp.text().await?;
        assert_eq!(text, format!("\"{expected_tx_id}\""));
        Ok(())
    }

    /// The bytes a client submits must reach the mempool untouched, so that what we later relay is
    /// byte-identical to what was submitted rather than to our own re-encoding of it.
    #[tokio::test]
    async fn test_submission_preserves_original_transaction_bytes() -> anyhow::Result<()> {
        let mempool: TestMempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default());
        let server = start_test_server_with_mempool(mempool.clone()).await?;
        let addr = server.address;

        let tx = create_transaction(0);
        let expected_tx_id = tx.tx_id();
        let body = non_canonical_transaction_cbor(&tx);

        let resp = submit_tx(addr, body.clone()).await?;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");
        assert_eq!(resp.text().await?, format!("\"{expected_tx_id}\""));

        let stored = mempool.get_tx(&expected_tx_id).expect("the submitted transaction in the mempool");
        assert_eq!(to_cbor(&stored), body);
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_cbor() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let resp = submit_tx(addr, vec![0xDE, 0xAD, 0xBE, 0xEF]).await?;
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_transaction() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let tx = create_transaction(0);
        let body = amaru_kernel::to_cbor(&tx);

        // First submission should succeed
        let resp = submit_tx(addr, body.clone()).await?;
        assert_eq!(resp.status(), 202);

        // Second submission should fail
        let resp = submit_tx(addr, body).await?;
        assert_eq!(resp.status(), 409);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_mempool_full() -> anyhow::Result<()> {
        let first = create_transaction(0);
        let second = create_transaction(1);

        let max_bytes = to_cbor(&first).len() as u64;
        let mempool: TestMempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::new(
            MempoolConfig::default().with_max_bytes(max_bytes),
        ));
        let server = start_test_server_with_mempool(mempool).await?;
        let addr = server.address;

        let resp = submit_tx(addr, amaru_kernel::to_cbor(&first)).await?;
        assert_eq!(resp.status(), 202);

        let resp = submit_tx(addr, amaru_kernel::to_cbor(&second)).await?;
        assert_eq!(resp.status(), 503);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_validation_failure() -> anyhow::Result<()> {
        let mempool: TestMempool = Arc::new(InMemoryMempool::new(Default::default()));
        let server = start_test_server_with_mempool_and_validator(mempool, Arc::new(reject_transactions)).await?;
        let addr = server.address;

        let tx = create_transaction(0);
        let resp = submit_tx(addr, amaru_kernel::to_cbor(&tx)).await?;
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(resp.text().await?, "transaction rejected for testing");
        Ok(())
    }

    #[tokio::test]
    async fn test_mempool_unavailable() -> anyhow::Result<()> {
        let mempool: TestMempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default());
        let (runtime, running) = make_mempool_runtime(mempool, Arc::new(MockCanValidateTxs));
        running.request_abort();
        running.termination().await;

        let tx = create_transaction(0);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/cbor".parse()?);
        let submitter = runtime.submitter();
        let resp = super::submit_tx(State(submitter), headers, Bytes::from(amaru_kernel::to_cbor(&tx))).await;
        assert_eq!(resp.status(), 503);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(to_bytes(resp.into_body(), usize::MAX).await?, "mempool unavailable");
        assert!(running.join().await?.unexpected_exits.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_wrong_content_type() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let resp = submit_tx_with_content_type(addr, "application/json", "{}").await?;
        assert_eq!(resp.status(), 415);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_content_type_with_charset_is_accepted() -> anyhow::Result<()> {
        let server = start_test_server().await?;
        let addr = server.address;

        let tx = create_transaction(0);
        let body = amaru_kernel::to_cbor(&tx);

        let resp = submit_tx_with_content_type(addr, "application/cbor; charset=binary", body).await?;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");
        Ok(())
    }

    /// This is a sanity check for the tests data
    #[test]
    fn decode_test_transaction() {
        let body = serialized_transaction().expect("a serialized transaction");
        let expected_tx: Transaction = minicbor::decode(&body).expect("a decoded transaction");
        let expected_tx_id = expected_tx.tx_id();
        assert_eq!(expected_tx_id.to_string(), TX_ID);
    }

    // HELPERS

    /// Re-encode a transaction's outer array header with a redundant one-byte length: still valid
    /// CBOR, but no longer byte-identical to what our encoder produces.
    fn non_canonical_transaction_cbor(tx: &WithOriginalBytes<Transaction>) -> Vec<u8> {
        let canonical = to_cbor(tx);
        assert_eq!(canonical[0], 0x84, "expected a definite-length array of 4 elements");
        [&[0x98, 0x04][..], &canonical[1..]].concat()
    }

    async fn start_test_server() -> anyhow::Result<TestSubmitApi> {
        let mempool: TestMempool = Arc::new(InMemoryMempool::<WithOriginalBytes<Transaction>>::default());
        start_test_server_with_mempool(mempool).await
    }

    async fn start_test_server_with_mempool(mempool: TestMempool) -> anyhow::Result<TestSubmitApi> {
        start_test_server_with_mempool_and_validator(mempool, Arc::new(MockCanValidateTxs)).await
    }

    async fn start_test_server_with_mempool_and_validator(
        mempool: TestMempool,
        validator: ResourceTxValidation,
    ) -> anyhow::Result<TestSubmitApi> {
        let (runtime, running) = make_mempool_runtime(mempool, validator);
        let shutdown = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse()?;
        let (server, address) = start(addr, runtime.submitter(), shutdown.clone()).await?;
        Ok(TestSubmitApi { address, shutdown, runtime, running, server })
    }

    fn make_mempool_runtime(
        mempool: TestMempool,
        validator: ResourceTxValidation,
    ) -> (Arc<MempoolRuntime>, TokioRunning) {
        use amaru_consensus::stages::mempool;

        let config = Config::default();
        let mut stage_graph = TokioBuilder::default().with_global_epoch_offset(config.compute_global_clock_offset());

        let mempool_stage = stage_graph.stage("mempool", mempool::stage);
        let mempool_stage = stage_graph.wire_up(mempool_stage, MempoolStageState::default());

        stage_graph.resources().put::<ResourceParameters>(config.global_parameters().clone());
        stage_graph.resources().put::<ResourceEraHistory>(config.era_history().clone());
        stage_graph.resources().put::<ResourceBlockValidation>(Arc::new(MockBlockValidator::default()));
        stage_graph.resources().put::<ResourceMempool<Transaction>>(mempool.clone());
        stage_graph.resources().put::<ResourceTxValidation>(validator);

        let sender = stage_graph.input(mempool_stage.without_state());
        let running = stage_graph.run(Handle::current());

        let runtime = MempoolRuntime::new(NodeRunId::new().unwrap(), mempool, sender, running.termination());
        (runtime, running)
    }

    fn reject_transactions(_tx: &Transaction) -> Result<(), TransactionValidationError> {
        Err(anyhow::anyhow!("transaction rejected for testing").into())
    }

    async fn submit_tx(addr: SocketAddr, body: impl Into<reqwest::Body>) -> anyhow::Result<Response> {
        submit_tx_with_content_type(addr, "application/cbor", body).await
    }

    async fn submit_tx_with_content_type(
        addr: SocketAddr,
        content_type: &str,
        body: impl Into<reqwest::Body>,
    ) -> anyhow::Result<Response> {
        reqwest::Client::new()
            .post(submit_tx_url(addr))
            .header("Content-Type", content_type)
            .body(body)
            .send()
            .await
            .map_err(Into::into)
    }

    fn submit_tx_url(addr: SocketAddr) -> String {
        format!("http://{addr}/api/submit/tx")
    }

    // This transaction is reconstructed from the transaction contained in the real preprod
    // block fixture `b9bef52dd8dedf992837d20c18399a284d80fde0ae9435f2a33649aaee7c5698`
    // (slot 70175999, block height 2671560).
    const TX_ID: &str = "43f396b0d5c55e34b507cfe9964672586370cc09912a4790488fba4079f96429";

    /// Return a serialized transaction extracted from an actual block
    fn serialized_transaction() -> anyhow::Result<Vec<u8>> {
        raw_block().transactions()?.next().ok_or_else(|| anyhow::anyhow!("no transactions found in block fixture"))
    }

    fn raw_block() -> RawBlock {
        const RAW_BLOCK: &[u8] = include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../amaru-kernel/tests/data/cbor.decode/block/",
            "b9bef52dd8dedf992837d20c18399a284d80fde0ae9435f2a33649aaee7c5698",
            "/sample.cbor"
        ));
        RawBlock::from(RAW_BLOCK)
    }
}
