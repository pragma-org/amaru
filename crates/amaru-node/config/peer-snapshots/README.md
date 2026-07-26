# Peer snapshots (build-time staging)

JSON peer snapshots for networks listed in `amaru_kernel::PEER_SNAPSHOT_NETWORKS`
(for example `mainnet`, `preprod`, `preview`) are staged here during `cargo build`:

```text
config/peer-snapshots/<network>/peer-snapshot.json
```

These files are **not** committed. The build script downloads them from
[cardano-foundation/cardano-configurations](https://github.com/cardano-foundation/cardano-configurations)
at the youngest commit at or before the Amaru `HEAD` committer timestamp.

If GitHub is unreachable, you can place files manually using the layout above and rebuild.
With `AMARU_PEER_SNAPSHOT_REQUIRED=1`, the build fails when any known network is still missing
(used for release CI).

After a successful fetch, `CONFIGS_COMMIT` records the configs-repo SHA used (also not committed).

Optional env vars:

| Variable | Effect |
|----------|--------|
| `AMARU_SKIP_PEER_SNAPSHOT_FETCH=1` | Do not contact GitHub; use staged files only |
| `AMARU_PEER_SNAPSHOT_REQUIRED=1` | Fail the build if any network file is missing |
| `GITHUB_TOKEN` / `GH_TOKEN` | Authenticate GitHub API (higher rate limits) |
