# Peer snapshots (build-time staging)

JSON peer snapshots for networks listed in `amaru_kernel::PEER_SNAPSHOT_NETWORKS`
(for example `mainnet`, `preprod`, `preview`) are staged here during `cargo build`:

```text
config/peer-snapshots/<network>/peer-snapshot.json
```

These files are **not** committed. The build script downloads them from
[cardano-foundation/cardano-configurations](https://github.com/cardano-foundation/cardano-configurations)
at the youngest commit at or before the Amaru `HEAD` committer timestamp.

The build **requires** a staged file for every known network. If GitHub is unreachable
and no files are present yet, the build fails with a message pointing here.

File format: [peer-snapshot.schema.json](./peer-snapshot.schema.json)
(compatible with cardano-node `peerSnapshotFile` / big-ledger peer snapshots).

## Offline / empty placeholders

When you cannot download real snapshots (air-gapped build, rate limits, etc.), create
minimal valid JSON files so the crate still builds. Empty `bigLedgerPools` is fine:
the binary simply starts without embedded big-ledger peers (you can still pass
`--peer-snapshot` at runtime).

Example for **preprod** (`NetworkMagic` is `1`):

```bash
mkdir -p config/peer-snapshots/preprod
cat > config/peer-snapshots/preprod/peer-snapshot.json <<'EOF'
{
  "NetworkMagic": 1,
  "NodeToClientVersion": 23,
  "Point": {
    "blockPointHash": "0000000000000000000000000000000000000000000000000000000000000000",
    "blockPointSlot": 0
  },
  "bigLedgerPools": []
}
EOF
```

Repeat for each required network, using the correct magic:

| Network  | `NetworkMagic` |
|----------|----------------|
| mainnet  | `764824073`    |
| preprod  | `1`            |
| preview  | `2`            |

Paths (from this crate root):

```text
config/peer-snapshots/mainnet/peer-snapshot.json
config/peer-snapshots/preprod/peer-snapshot.json
config/peer-snapshots/preview/peer-snapshot.json
```

## Conditional fetch cache

After a successful commits-API response, `CONFIGS_COMMIT_CACHE` records the configs-repo
SHA, the Amaru HEAD committer time used as GitHub `until` (unix seconds), plus any
`ETag` / `Last-Modified` headers (also not committed), and gets a fresh modification
time. Builds reuse that cached SHA without contacting the commits API only when
**both** hold:

- the cache file is younger than **12 hours** (avoids rate-limit noise on frequent
  local/LSP rebuilds), and
- the currently required Amaru HEAD committer time is within **1 hour** of the
  cached `until` (so checking out a much older/newer commit re-resolves the
  configs SHA instead of reusing a mismatched snapshot).

When a refresh runs, later builds send conditional requests if `until` is still
compatible; a `304 Not Modified` rewrites the cache (refreshing the 12h window) and
only re-downloads missing or stale snapshot files.

## Optional env vars

| Variable | Effect |
|----------|--------|
| `AMARU_SKIP_PEER_SNAPSHOT_FETCH=1` | Do not contact GitHub; use staged files only |
| `GITHUB_TOKEN` / `GH_TOKEN` | Authenticate GitHub API (higher rate limits; 304s do not count against the primary limit) |

## CI staging

GitHub Actions that compile Amaru use the composite action
[`.github/actions/stage-peer-snapshots`](../../../../.github/actions/stage-peer-snapshots)
(wrapping [`scripts/stage-peer-snapshots`](../../../../scripts/stage-peer-snapshots)):
authenticated curl fetch first, then cargo with `AMARU_SKIP_PEER_SNAPSHOT_FETCH=1`
and blank `GITHUB_TOKEN` / `GH_TOKEN` so build scripts never see credentials.

You can run the same script locally:

```bash
# optional: export GITHUB_TOKEN=…
./scripts/stage-peer-snapshots
AMARU_SKIP_PEER_SNAPSHOT_FETCH=1 cargo build -p amaru-node
```
