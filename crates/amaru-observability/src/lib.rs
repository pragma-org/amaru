pub const OPENING_CHAIN_DB: &str = "opening chain db";
pub const MIGRATING_DATABASE: &str = "migrating database";

pub mod registry;

// Re-export the macros for convenient use
pub use amaru_observability_macros::{augment_trace, define_schemas, trace};

pub mod stores {

    pub const SNAPSHOT: &str = "snapshot";
    pub const DREPS_DELEGATION_REMOVE: &str = "dreps_delegations.remove";

    pub mod consensus {
        pub const STORE_BLOCK: &str = "consensus.store.store_block";
        pub const STORE_HEADER: &str = "consensus.store.store_header";
        pub const ROLL_FORWARD_CHAIN: &str = "consensus.store.roll_forward_chain";
        pub const ROLLBACK_CHAIN: &str = "consensus.store.rollback_chain";
    }
}
