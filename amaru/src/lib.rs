/// Sync pipeline
///
/// The sync pipeline is responsible for fetching blocks from the upstream node and
/// applying them to the local chain.
pub mod sync;

/// Ledger interface
///
/// A preliminary ledger implementation in memory. It's primarily meant for demo purpose
/// and will very likely move into its own crate eventually.
pub mod ledger;
