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
use amaru_ouroboros::{ResourceMempool, TxInsertResult, TxOrigin, TxRejectReason};
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
use tracing::{info, warn};

/// For now we just need access to the mempool to route requests from the tx submission API.
type SubmitApiState = ResourceMempool<Transaction>;

/// Starts the Submit API server on the specified address, using the provided mempool
/// for transaction submission.
pub async fn start(
    addr: SocketAddr,
    mempool: ResourceMempool<Transaction>,
    shutdown: CancellationToken,
) -> anyhow::Result<(JoinHandle<()>, SocketAddr)> {
    let app = Router::new().route("/api/submit/tx", post(submit_tx)).with_state(mempool);

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
async fn submit_tx(State(mempool): State<SubmitApiState>, headers: HeaderMap, body: Bytes) -> Response {
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

    match mempool.insert(tx, TxOrigin::Local) {
        Ok(TxInsertResult::Accepted { tx_id, .. }) => json_response(StatusCode::ACCEPTED, tx_id.to_string()),
        Ok(TxInsertResult::Rejected(reason)) => text_response(
            match reason {
                TxRejectReason::MempoolFull => StatusCode::SERVICE_UNAVAILABLE,
                TxRejectReason::Duplicate => StatusCode::CONFLICT,
                TxRejectReason::Invalid(_) => StatusCode::BAD_REQUEST,
            },
            reason.to_string(),
        ),
        Err(error) => text_response(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
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

    use amaru_kernel::{RawBlock, Transaction};
    use amaru_mempool::{InMemoryMempool, MempoolConfig};
    use amaru_ouroboros_traits::{TransactionValidationError, TxId};
    use reqwest::{Response, header::CONTENT_TYPE};
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
        let mempool: Arc<dyn amaru_ouroboros_traits::TxSubmissionMempool<Transaction>> =
            Arc::new(InMemoryMempool::<Transaction>::from_config(MempoolConfig::default().with_max_txs(1)));
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
            Arc::new(InMemoryMempool::new(Default::default(), Arc::new(reject_transactions)));
        let (addr, _shutdown) = start_test_server_with_mempool(mempool).await?;

        let tx = create_transaction(0);
        let resp = submit_tx(addr, amaru_kernel::to_cbor(&tx)).await?;
        assert_eq!(resp.status(), 400);
        assert_eq!(resp.headers()[CONTENT_TYPE], "text/plain; charset=utf-8");
        assert_eq!(resp.text().await?, "TransactionValidationError: transaction rejected for testing");
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
        let mempool: Arc<dyn amaru_ouroboros_traits::TxSubmissionMempool<Transaction>> =
            Arc::new(InMemoryMempool::<Transaction>::default());
        start_test_server_with_mempool(mempool).await
    }

    async fn start_test_server_with_mempool(
        mempool: Arc<dyn amaru_ouroboros_traits::TxSubmissionMempool<Transaction>>,
    ) -> anyhow::Result<(SocketAddr, CancellationToken)> {
        let shutdown = CancellationToken::new();
        let addr: SocketAddr = "127.0.0.1:0".parse()?;
        let (_handle, local_addr) = start(addr, mempool, shutdown.clone()).await?;
        Ok((local_addr, shutdown))
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
}
