# Bootstrap Snapshots

Amaru bootstrap expects a window of three consecutive epoch snapshots. The runtime reads that window from `crates/amaru/config/bootstrap/<network>/snapshots.json`, and each entry points to one compressed archive in `snapshots/<network>/`.

## Create a Snapshot Set

Generate a bootstrap set by passing the first epoch in the three-epoch window to `generate-epoch-snapshots`. For example, this creates the set for epochs `163`, `164`, and `165` on `preprod`:

```shell
cargo run generate-epoch-snapshots --network preprod --epoch 163
```

The command expands the requested epoch to three consecutive targets, fetches the last block of each epoch, downloads or resumes the backing cardano-db from Mithril, runs `db-analyser` in Docker, and then writes:

* metadata files under `data/<network>/epoch-snapshots/epochs/`
* compressed snapshot archives under `snapshots/<network>/`

## Publish a Snapshot Set

Publishing uploads the three generated archives to an S3-compatible bucket and rewrites `crates/amaru/config/bootstrap/<network>/snapshots.json` so bootstrap clients can fetch them.

Set the required environment first:

```shell
export AWS_ACCESS_KEY_ID=...
export AWS_SECRET_ACCESS_KEY=...
export AWS_DEFAULT_REGION=auto
export BUCKET_NAME=...
export ENDPOINT=https://<s3-compatible-endpoint>
export BOOTSTRAP_SNAPSHOT_PUBLIC_URL_BASE=https://<public-base-url>
```

The publish script also requires `aws` and `jq` on `PATH`.

Once the archives already exist locally, publish them with the first epoch in the window:

```shell
make \
	AMARU_NETWORK=preprod \
	BOOTSTRAP_SNAPSHOT_EPOCH=163 \
	BUCKET_NAME=... \
	ENDPOINT=https://<s3-compatible-endpoint> \
	BOOTSTRAP_SNAPSHOT_PUBLIC_URL_BASE=https://<public-base-url> \
	publish-bootstrap-snapshots
```

Commit the updated `snapshots.json` when you want that new three-epoch window to become the default bootstrap set for the selected network.
