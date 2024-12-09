/// Sync pipeline
///
/// The sync pipeline is responsible for fetching blocks from the upstream node and
/// applying them to the local chain.
pub mod sync;

/// Consensus interface
///
/// The consensus interface is responsible for validating block headers.
pub mod consensus;

/// Ledger interface
///
/// A preliminary ledger implementation in memory. It's primarily meant for demo purpose
/// and will very likely move into its own crate eventually.
pub mod ledger;

/// Iterators
///
/// A set of additional primitives around iterators. Not Amaru-specific so-to-speak.
pub mod iter;
