# Publishing bootstrap snapshots

Amaru nodes bootstrap from three consecutive epoch snapshots. They are stored
in a public R2 bucket and listed in `<network>/index.json` in that bucket. See
[BOOTSTRAP.md](./BOOTSTRAP.md) for how snapshots are built.

## How to publish

1. Go to Actions → "Publish Bootstrap Snapshots" → "Run workflow".
2. Pick a network. Leave `epoch` empty to use the latest network epoch as the
   bootstrap target, or set the target epoch explicitly.
3. Optionally set `cardano_node_version` to a specific cardano-node release
   tag (default: `11.0.1`). Must be a published release tag, not a commit hash.
4. The run publishes the archives, updates the bucket index and verifies a
   full bootstrap against the published snapshots.

## What the workflow does

For each selected network:

1. `amaru snapshot create`: downloads chain data from Mithril, replays it
   with `db-analyser` and packs three epoch snapshots into `.tar.zst` archives.
2. `amaru snapshot publish`: uploads the archives and updates the network's
   `index.json` in the bucket.
3. Runs `amaru node bootstrap` in a temporary directory using the published
   index. This tests the same path end users take. Skip it with
   `skip_verification`.

Chain data stays on the runner between runs, so only the first run is slow.
Running the workflow again for the same epoch is safe: existing archives are
not uploaded twice.

If a run fails with "db-analyser did not create snapshot directory", the
latest epoch is not yet available from Mithril. This can happen shortly
after an epoch boundary, mostly on preview. Try again a few hours later.

## Runner setup

The workflow needs a self-hosted runner with the labels `self-hosted` and
`snapshots`.

- Linux or macOS, x86_64 or arm64.
- Disk: about 50 GB per testnet, 500 GB for mainnet.
- On `$PATH`: `rustup`, `curl`, `git`.
- `db-analyser` is always downloaded fresh from the cardano-node release
  specified by `cardano_node_version`.

If the runner runs as a service, make sure those tools are on the service's
`PATH` (the runner reads a `.path` file from its directory).

Work files are kept in `$HOME/amaru-snapshots/<network>/`, or in
`SNAPSHOTS_CACHE_DIR` when set:

- `dist/`: chain data and ledger snapshots. Keep it; it makes runs fast.
- `snapshots/`: the generated archives. Old ones can be deleted once
  published.

### Secrets and variables

| Name | Kind | Purpose |
|------|------|---------|
| `R2_ACCESS_KEY_ID` / `R2_SECRET_ACCESS_KEY` | secret | R2 credentials with write access to the bucket |
| `S3_ENDPOINT` | secret | R2 S3 endpoint |
| `SNAPSHOTS_BUCKET_NAME` | secret | Bucket to upload to |
| `SNAPSHOTS_CACHE_DIR` | variable, optional | Cache directory on the runner |
| `SNAPSHOTS_PUBLIC_URL_BASE` | variable or secret | Public URL of the bucket (required) |

### Security

Amaru is a public repository, so lock the runner down:

- Put it in a runner group limited to this repository.
- Keep this workflow manual-only. Never add `pull_request` triggers to
  workflows that use the `snapshots` label.
- Require approval for workflow runs from outside collaborators.

## Testing in a fork

1. Push the branch and get the workflow file onto the fork's default branch.
2. Create a bucket with public read access and set the secrets above.
3. Set `SNAPSHOTS_PUBLIC_URL_BASE` to your bucket's public URL.
4. Register a runner with the `snapshots` label.
5. Run the workflow for `preview`, the smallest network.

## Running it by hand

The same steps work on any machine that meets the requirements above:

```shell
cargo run --release --bin amaru -- snapshot create --network preprod

AWS_ACCESS_KEY_ID=... \
AWS_SECRET_ACCESS_KEY=... \
cargo run --release --bin amaru -- snapshot publish --network preprod
```
