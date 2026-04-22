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

use amaru_kernel::Transaction;
use amaru_ouroboros::{TxInsertResult, TxOrigin, TxRejectReason};
use amaru_ouroboros_traits::MempoolMsg;
use amaru_protocols::tx_submission::MEMPOOL_INSERT_TIMEOUT;
use anyhow::Context;
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use pure_stage::{CallError, Sender};
use tokio::{net::TcpListener, task::JoinHandle};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// For now we just need access to the mempool to route requests from the tx submission API.
type SubmitApiState = Sender<MempoolMsg>;

/// Starts the Submit API server on the specified address, using the provided mempool
/// for transaction submission.
pub async fn start(
    addr: SocketAddr,
    mempool_sender: Sender<MempoolMsg>,
    shutdown: CancellationToken,
) -> anyhow::Result<(JoinHandle<()>, SocketAddr)> {
    let app = Router::new().route("/api/submit/tx", post(submit_tx)).with_state(mempool_sender);

    let listener =
        TcpListener::bind(addr).await.with_context(|| format!("failed to bind submit API address at {addr}"))?;
    let local_addr = listener.local_addr().context("failed to get local address")?;

    info!(%local_addr, "Submit API server started");

    let handle = tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, app).with_graceful_shutdown(shutdown.cancelled_owned()).await {
            warn!(error = %err, "Submit API server stopped with error");
        }
    });

    Ok((handle, local_addr))
}

/// Handle incoming transaction submission requests.
/// The request body is expected to be a CBOR-encoded.
async fn submit_tx(State(mempool_sender): State<SubmitApiState>, headers: HeaderMap, body: Bytes) -> Response {
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

    let tx: Transaction = match minicbor::decode(&body) {
        Ok(tx) => tx,
        Err(e) => {
            return text_response(StatusCode::BAD_REQUEST, format!("Invalid CBOR transaction: {e}"));
        }
    };

    match mempool_sender
        .call(|caller| MempoolMsg::Insert { tx: Box::new(tx), origin: TxOrigin::Local, caller }, MEMPOOL_INSERT_TIMEOUT)
        .await
    {
        Ok(Ok(TxInsertResult::Accepted { tx_id, .. })) => json_response(StatusCode::ACCEPTED, tx_id.to_string()),
        Ok(Ok(TxInsertResult::Rejected { reason, .. })) => text_response(
            match reason {
                TxRejectReason::MempoolFull => StatusCode::SERVICE_UNAVAILABLE,
                TxRejectReason::Duplicate => StatusCode::CONFLICT,
                TxRejectReason::Invalid(_) => StatusCode::BAD_REQUEST,
            },
            reason.to_string(),
        ),
        Ok(Err(error)) => {
            warn!(tx_id = %error.tx_id, error = %error.error, "mempool insert failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool unavailable")
        }
        Err(CallError::TimedOut) => text_response(StatusCode::SERVICE_UNAVAILABLE, "mempool timed out"),
        Err(CallError::SendFailed) => {
            warn!("mempool send failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool unavailable")
        }
        Err(CallError::ResponseDropped) => {
            warn!("mempool response dropped");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool unavailable")
        }
        Err(CallError::ResponseDeserializeFailed) => {
            warn!("mempool response deserialization failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "mempool returned an invalid response")
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
    use std::{
        net::SocketAddr,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use amaru_consensus::{effects::ResourceTxValidation, stages::mempool::MempoolStageState};
    use amaru_kernel::{RawBlock, Transaction};
    use amaru_mempool::{InMemoryMempool, MempoolConfig};
    use amaru_ouroboros::{MempoolMsg, ResourceMempool};
    use amaru_ouroboros_traits::{
        MempoolError, MempoolSeqNo, MockCanValidateTxs, TransactionValidationError, TxId, TxInsertResult, TxOrigin,
        TxSubmissionMempool,
    };
    use axum::{
        body::{Bytes, to_bytes},
        extract::State,
        http::HeaderMap,
    };
    use pure_stage::{
        Sender, StageGraph,
        tokio::{TokioBuilder, TokioRunning},
    };
    use reqwest::{Response, header::CONTENT_TYPE};
    use tokio::runtime::Handle;
    use tokio_util::sync::CancellationToken;

    use super::start;
    use crate::tests::test_data::create_transaction;

    #[tokio::test]
    async fn test_successful_submission() -> anyhow::Result<()> {
        let (addr, _shutdown) = start_test_server().await?;

        let tx = create_transaction(0);
        let expected_tx_id = TxId::from(&tx);
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
        let (addr, _shutdown) = start_test_server().await?;

        let body = serialized_transaction()?;
        let expected_tx: Transaction = minicbor::decode(&body)?;
        let expected_tx_id = TxId::from(&expected_tx);
        assert_eq!(expected_tx_id.to_string(), TX_ID);

        let resp = submit_tx(addr, body).await?;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.headers()[CONTENT_TYPE], "application/json");

        let text = resp.text().await?;
        assert_eq!(text, format!("\"{expected_tx_id}\""));
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_cbor() -> anyhow::Result<()> {
        let (addr, _shutdown) = start_test_server().await?;

        let resp = submit_tx(addr, vec![0xDE, 0xAD, 0xBE, 0xEF]).await?;
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_transaction() -> anyhow::Result<()> {
        let (addr, _shutdown) = start_test_server().await?;

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
        let mempool: Arc<dyn TxSubmissionMempool<Transaction>> =
            Arc::new(InMemoryMempool::<Transaction>::new(MempoolConfig::default().with_max_txs(1)));
        let (addr, _shutdown) = start_test_server_with_mempool(mempool).await?;

        let first = create_transaction(0);
        let second = create_transaction(1);

        let resp = submit_tx(addr, amaru_kernel::to_cbor(&first)).await?;
        assert_eq!(resp.status(), 202);

        let resp = submit_tx(addr, amaru_kernel::to_cbor(&second)).await?;
        assert_eq!(resp.status(), 503);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_validation_failure() -> anyhow::Result<()> {
        let mempool: Arc<dyn amaru_ouroboros_traits::TxSubmissionMempool<Transaction>> =
            Arc::new(InMemoryMempool::new(Default::default()));
        let (addr, _shutdown) =
            start_test_server_with_mempool_and_validator(mempool, Arc::new(reject_transactions)).await?;

        let tx = create_transaction(0);
        let resp = submit_tx(addr, amaru_kernel::to_cbor(&tx)).await?;
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(resp.text().await?, "transaction rejected for testing");
        Ok(())
    }

    #[tokio::test]
    async fn test_mempool_unavailable() -> anyhow::Result<()> {
        let mempool: Arc<dyn TxSubmissionMempool<Transaction>> = Arc::new(InMemoryMempool::<Transaction>::default());
        let (sender, running) = make_mempool_sender_and_running(mempool, Arc::new(MockCanValidateTxs));
        running.abort();

        let tx = create_transaction(0);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/cbor".parse()?);
        let resp = super::submit_tx(State(sender), headers, Bytes::from(amaru_kernel::to_cbor(&tx))).await;
        assert_eq!(resp.status(), 500);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(to_bytes(resp.into_body(), usize::MAX).await?, "mempool unavailable");
        Ok(())
    }

    #[tokio::test]
    async fn test_mempool_stage_survives_insert_failure() -> anyhow::Result<()> {
        let mempool: Arc<dyn TxSubmissionMempool<Transaction>> = Arc::new(FailOnceMempool::default());
        let first = create_transaction(0);
        let second = create_transaction(1);
        let sender = make_mempool_sender(mempool, Arc::new(MockCanValidateTxs));
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/cbor".parse()?);

        let resp =
            super::submit_tx(State(sender.clone()), headers.clone(), Bytes::from(amaru_kernel::to_cbor(&first))).await;
        assert_eq!(resp.status(), 500);
        assert_eq!(to_bytes(resp.into_body(), usize::MAX).await?, "mempool unavailable");

        let resp = super::submit_tx(State(sender), headers, Bytes::from(amaru_kernel::to_cbor(&second))).await;
        assert_eq!(resp.status(), 202);
        Ok(())
    }

    #[tokio::test]
    async fn test_wrong_content_type() -> anyhow::Result<()> {
        let (addr, _shutdown) = start_test_server().await?;

        let resp = submit_tx_with_content_type(addr, "application/json", "{}").await?;
        assert_eq!(resp.status(), 415);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        Ok(())
    }

    #[tokio::test]
    async fn test_content_type_with_charset_is_accepted() -> anyhow::Result<()> {
        let (addr, _shutdown) = start_test_server().await?;

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
        let expected_tx_id = TxId::from(&expected_tx);
        assert_eq!(expected_tx_id.to_string(), TX_ID);
    }

    // HELPERS

    async fn start_test_server() -> anyhow::Result<(SocketAddr, CancellationToken)> {
        let mempool: Arc<dyn TxSubmissionMempool<Transaction>> = Arc::new(InMemoryMempool::<Transaction>::default());
        start_test_server_with_mempool(mempool).await
    }

    async fn start_test_server_with_mempool(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
    ) -> anyhow::Result<(SocketAddr, CancellationToken)> {
        start_test_server_with_mempool_and_validator(mempool, Arc::new(MockCanValidateTxs)).await
    }

    async fn start_test_server_with_mempool_and_validator(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        validator: ResourceTxValidation,
    ) -> anyhow::Result<(SocketAddr, CancellationToken)> {
        let sender = make_mempool_sender(mempool, validator);
        let shutdown = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse()?;
        let (_handle, local_addr) = start(addr, sender, shutdown.clone()).await?;
        Ok((local_addr, shutdown))
    }

    fn make_mempool_sender(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        validator: ResourceTxValidation,
    ) -> Sender<MempoolMsg> {
        make_mempool_sender_and_running(mempool, validator).0
    }

    fn make_mempool_sender_and_running(
        mempool: Arc<dyn TxSubmissionMempool<Transaction>>,
        validator: ResourceTxValidation,
    ) -> (Sender<MempoolMsg>, TokioRunning) {
        use amaru_consensus::stages::mempool;
        let mut stage_graph = TokioBuilder::default();
        let mempool_stage = stage_graph.stage("mempool", mempool::stage);
        let mempool_stage = stage_graph.wire_up(mempool_stage, MempoolStageState::default());
        stage_graph.resources().put::<ResourceMempool<Transaction>>(mempool);
        stage_graph.resources().put::<ResourceTxValidation>(validator);
        let sender = stage_graph.input(mempool_stage.without_state());
        let running = stage_graph.run(Handle::current());
        (sender, running)
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
    const TX_ID: &str = "7a53dd0382932042d1e7518ac85caa7beedcbff57a6a206140b63fa74cd27706";

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

    #[derive(Default)]
    struct FailOnceMempool {
        attempts: AtomicUsize,
    }

    impl TxSubmissionMempool<Transaction> for FailOnceMempool {
        fn insert(&self, tx: Transaction, _tx_origin: TxOrigin) -> Result<TxInsertResult, MempoolError> {
            if self.attempts.fetch_add(1, Ordering::Relaxed) == 0 {
                Err(MempoolError::new("temporary failure"))
            } else {
                Ok(TxInsertResult::accepted(TxId::from(&tx), MempoolSeqNo(1)))
            }
        }

        fn get_tx(&self, _tx_id: &TxId) -> Option<Transaction> {
            None
        }

        fn tx_ids_since(&self, _from_seq: MempoolSeqNo, _limit: u16) -> Vec<(TxId, u32, MempoolSeqNo)> {
            vec![]
        }

        fn get_txs_for_ids(&self, _ids: &[TxId]) -> Vec<Transaction> {
            vec![]
        }

        fn last_seq_no(&self) -> MempoolSeqNo {
            MempoolSeqNo(0)
        }
    }
}
