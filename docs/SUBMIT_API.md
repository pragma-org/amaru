# Submit API

The Submit API provides an HTTP endpoint for submitting transactions to Amaru's mempool. It follows the [Cardano Submit API](https://input-output-hk.github.io/cardano-rest/submit-api/) standard used by wallets like Eternl.

## Enabling

Use the `--submit-api-address` CLI flag or the `AMARU_SUBMIT_API_ADDRESS` environment variable.

Minimal working example (assuming the default preprod network and default peer at `127.0.0.1:3001`):

```bash
amaru bootstrap
amaru run --submit-api-address 127.0.0.1:8090
```

## API Reference

### POST /api/submit/tx

Submit a CBOR-encoded Cardano transaction.

**Request:**
- `Content-Type: application/cbor` (required)
- Body: raw CBOR-encoded transaction bytes

**Responses:**

| Status | Content-Type | Body |
|--------|-------------|------|
| 202 Accepted | application/json | Hex-encoded transaction ID (64 chars), e.g. `"abc123..."` |
| 400 Bad Request | text/plain | Error message for invalid CBOR or failed validation |
| 409 Conflict | text/plain | `Transaction is a duplicate` |
| 503 Service Unavailable | text/plain | `Mempool is full` |
| 415 Unsupported Media Type | text/plain | `Content-Type must be application/cbor` |

## Example

```bash
curl -X POST \
  --header "Content-Type: application/cbor" \
  --data-binary @tx.cbor \
  http://localhost:8090/api/submit/tx
```

## Error Responses

- **400 — Invalid CBOR transaction**: The request body could not be decoded as a valid Cardano transaction.
- **400 — Transaction validation failed**: The transaction was rejected by the node's transaction validator.
- **409 — Transaction is a duplicate**: The transaction already exists in the mempool.
- **503 — Mempool is full**: The mempool has reached its capacity limit; callers can retry later.
- **415 — Unsupported Media Type**: The `Content-Type` header is missing or not `application/cbor`.
