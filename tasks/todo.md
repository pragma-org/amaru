# Defer header validation when stake distribution is not yet available

## Background

Issue: at startup with peer way ahead of ledger tip, header validation fails with
`no stake distribution available for pool access {epoch}` because the required
distribution can only be computed after the ledger applies a block past stability
window in the following epoch. The peer is then dropped (treated as if it sent a
bad header).

## Plan (approach A — defer current header validation)

- [x] **Step 1**: Plumb the structured "stake distribution not available" error
  end-to-end so callers can detect it.
  - [x] Add `missing_stake_distribution: Option<Epoch>` discriminant to
    `HeaderValidationError` (amaru-ouroboros-traits).
  - [x] Add `HeaderValidationError::missing_stake_distribution(epoch)` constructor
    and `as_missing_stake_distribution(&self) -> Option<Epoch>` accessor.
  - [x] In `ValidateHeader::check_header` (validate_header.rs:86), branch on
    `AssertHeaderError::PoolError(GetPoolError::StakeDistributionNotAvailable(e))`
    to construct the structured variant.
  - [x] In `CanValidateHeaders for ValidateHeader::validate_header`
    (validate_header.rs:91-93), pass through the inner `HeaderValidationError`
    instead of re-wrapping via `anyhow!`.
- [x] **Step 2**: in `TrackPeers`, do not drop peer on transient error.
  - [x] Add `missing_stake_distribution(&ConsensusError) -> Option<Epoch>` helper.
  - [x] In `execute_roll_forward`, branch on transient: log at WARN, leave
    `per_peer` untouched, return without removing the peer.
  - [x] Peer's chainsync will wedge on the next RollForward until step 3 is in
    place. This is intentional for an isolated step-2 review.
- [ ] **Step 3** (later): new `defer_validation` stage that re-injects pending
  RollForward messages once the ledger reaches the required slot.
  - [ ] Move pre-validation `RequestNext` so we don't race ahead while pending.
  - [ ] Add new test exercising the transient-error → retry → success path.
