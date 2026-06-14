# Publishing bootstrap snapshots

Amaru nodes bootstrap from three consecutive epoch snapshots. They are stored
in a public R2 bucket and listed in
`crates/amaru/config/bootstrap/<network>/snapshots.json`. See
[BOOTSTRAP.md](./BOOTSTRAP.md) for how snapshots are built.

## How to publish

1. Go to Actions → "Publish Bootstrap Snapshots" → "Run workflow".
2. Pick a network. Leave `epoch` empty to use the last three completed
   epochs, or set the first epoch of the set you want.
3. Optionally set `cardano_node_version` to a specific cardano-node release
   tag (default: `11.0.1`). Must be a published release tag, not a commit hash.
4. The run opens a pull request that updates `snapshots.json`. Review and
   merge it.

The PR is opened with `GITHUB_TOKEN`. This needs a one-time setting:
Settings → Actions → General → Workflow permissions → enable "Allow GitHub
Actions to create and approve pull requests".

`GITHUB_TOKEN` cannot trigger `pull_request` workflows, so the workflow
starts CI on the branch itself. To re-run CI, close and reopen the PR.

## What the workflow does

For each selected network:

1. `amaru create-snapshots`: downloads chain data from Mithril, replays it
   with `db-analyser` and packs three epoch snapshots into tar.gz archives.
2. `scripts/publish-bootstrap-snapshots`: uploads the archives, checks they
   can be downloaded publicly and updates `snapshots.json`.
3. Runs `amaru bootstrap` in a temporary directory using the new
   `snapshots.json`. This tests the same path end users take. Skip it with
   `skip_verification`.
4. Opens the pull request.

Chain data stays on the runner between runs, so only the first run is slow.
Running the workflow again for the same epoch is safe: nothing is uploaded
twice and no PR is opened if nothing changed.

If a run fails with "db-analyser did not create snapshot directory", the
latest epoch is not yet available from Mithril. This can happen shortly
after an epoch boundary, mostly on preview. Try again a few hours later.

## Runner setup

The workflow needs a self-hosted runner with the labels `self-hosted` and
`snapshots`.

- Linux or macOS, x86_64 or arm64.
- Disk: about 50 GB per testnet, 500 GB for mainnet.
- On `$PATH`: `rustup`, `aws`, `jq`, `curl`, `gh`, `git`.
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
| `SNAPSHOTS_PUBLIC_URL_BASE` | variable, optional | Public URL of the bucket; defaults to the base of the URLs already in `snapshots.json` |

### Security

Amaru is a public repository, so lock the runner down:

- Put it in a runner group limited to this repository.
- Keep this workflow manual-only. Never add `pull_request` triggers to
  workflows that use the `snapshots` label.
- Require approval for workflow runs from outside collaborators.

## Testing in a fork

1. Push the branch and get the workflow file onto the fork's default branch.
2. Create a bucket with public read access and set the secrets above.
3. Set `SNAPSHOTS_PUBLIC_URL_BASE` to your bucket's public URL. Without it
   the script checks the upstream URLs instead and skips your uploads.
4. Enable the PR permission setting mentioned above.
5. Register a runner with the `snapshots` label.
6. Run the workflow for `preview`, the smallest network.

## Running it by hand

The same steps work on any machine that meets the requirements above:

```shell
cargo run --release --bin amaru -- create-snapshots --network preprod -f

AWS_ACCESS_KEY_ID=... \
AWS_SECRET_ACCESS_KEY=... \
AWS_DEFAULT_REGION=auto \
BUCKET_NAME=... \
ENDPOINT=... \
AMARU_NETWORK=preprod \
bash ./scripts/publish-bootstrap-snapshots
```

Then commit the `snapshots.json` change and open a PR.
