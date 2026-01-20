pub const OPENING_CHAIN_DB: &str = "opening chain db";
pub const MIGRATING_DATABASE: &str = "migrating database";

pub mod consensus {

    pub mod diffusion {
        pub const FETCH_BLOCK: &str = "diffusion.fetch_block";
        pub const FORWARD_CHAIN: &str = "diffusion.forward_chain";
        pub const CHAIN_SYNC: &str = "diffusion.chain_sync";
        pub const CHAIN_SYNC_WAIT: &str = "diffusion.chain_sync.wait";
    }

    pub mod chain_sync {
        pub const DECODE_HEADER: &str = "chain_sync.decode_header";
        pub const RECEIVE_HEADER: &str = "chain_sync.receive_header";
        pub const RECEIVE_HEADER_DECODE_FAILED: &str = "chain_sync.receive_header.decode_failed";
        pub const SELECT_CHAIN: &str = "chain_sync.select_chain";
        pub const TRACK_PEERS: &str = "chain_sync.track_peers";
        pub const VALIDATE_BLOCK: &str = "chain_sync.validate_block";
        pub const VALIDATE_HEADER: &str = "chain_sync.validate_header";
    }

    pub mod validate_header {
        pub const EVOLVE_NONCE: &str = "validate_header.evolve_nonce";
        pub const VALIDATE: &str = "validate_header.validate";
    }
}

pub mod ledger {
    pub const PREPARE_BLOCK: &str = "ledger.prepare_block";
    pub const PARSE_BLOCK: &str = "ledger.parse_block";
    pub const VALIDATE_BLOCK: &str = "ledger.validate_block";
    pub const ADD_FEES: &str = "add_fees";
    pub const WITHDRAW_FROM: &str = "withdraw_from";
    pub const VOTE: &str = "vote";
    pub const REQUIRE_VKEY_WITNESS: &str = "require_vkey_witness";
    pub const REQUIRE_SCRIPT_WITNESS: &str = "require_script_witness";
    pub const REQUIRE_BOOTSTRAP_WITNESS: &str = "require_bootstrap_witness";
    pub const STATE_RESOLVE_INPUTS: &str = "state.resolve_inputs";
    pub const CREATE_VALIDATION_CONTEXT: &str = "ledger.create_validation_context";
    pub const ROLL_FORWARD: &str = "ledger.roll_forward";
    pub const ROLL_BACKWARD: &str = "ledger.roll_backward";
    pub const RESET_FEES: &str = "reset.fees";
    pub const RESET_BLOCKS_COUNT: &str = "reset.blocks_count";
    pub const TICK_POOL: &str = "tick.pool";
    pub const TICK_PROPOSALS: &str = "tick.proposals";
    pub const RATIFICATION_CONTEXT_NEW: &str = "ratification.context.new";
    pub const ENACTING: &str = "enacting";
    pub const RATIFYING: &str = "ratifying";
}

pub mod network {

    pub mod chainsync_client {
        pub const FIND_INTERSECTION: &str = "chainsync_client.find_intersection";
    }
}

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

// TODO
// Common naming schema?
// all at pure-stage binding level?
