# Peer snapshots (build-time staging)

Optional **offline / CI inputs** for networks listed in `amaru_kernel::PEER_SNAPSHOT_NETWORKS`
(for example `mainnet`, `preprod`, `preview`):

```text
config/peer-snapshots/<network>/peer-snapshot.json
```

These files are **not** committed. The build script embeds peer snapshots into `OUT_DIR`
(and may download them there from
[cardano-foundation/cardano-configurations](https://github.com/cardano-foundation/cardano-configurations)
when GitHub is reachable). Staged files under this directory are only a fallback when
fetch is skipped or fails.

Cargo is told to re-run the build script when these staged files change. Fetch metadata
and downloaded bytes live only under `OUT_DIR`, so a successful fetch does **not** dirty
the package tree or force an `amaru-node` rebuild on the next `cargo build`.

The build **requires** embeddable bytes for every known network (from download and/or
staged files). If GitHub is unreachable and no staged files are present yet, the build
fails with a message pointing here.

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

## Conditional fetch cache (OUT_DIR only)

After a successful commits-API response, `OUT_DIR/CONFIGS_COMMIT_CACHE` records the
configs-repo SHA, the Amaru HEAD committer time used as GitHub `until` (unix seconds),
plus any `ETag` / `Last-Modified` headers. That file is **not** a Cargo rebuild input.

While the cache is younger than **12 hours** and the required Amaru HEAD committer time
is within **1 hour** of the cached `until`, later build-script runs reuse the cached SHA
without contacting the commits API. Snapshot files already present in `OUT_DIR` for that
SHA are not re-downloaded.

## Optional env vars

| Variable | Effect |
|----------|--------|
| `AMARU_SKIP_PEER_SNAPSHOT_FETCH=1` | Do not contact GitHub; use staged files / existing `OUT_DIR` bytes only |
| `GITHUB_TOKEN` / `GH_TOKEN` | Authenticate GitHub API (higher rate limits; 304s do not count against the primary limit) |

These are **not** listed as `cargo:rerun-if-env-changed`. Flipping a token or skip flag
alone will not recompile `amaru-node`; they are consulted only when the build script runs
for another reason (clean build, staged-file change, build-script source change, etc.).

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
