// Copyright 2024 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TODO: Temporary re-exports until Pallas migrations
//
// Re-exports still needed in a few places; but that shall become redundant as soon as we have
// properly reworked addresses.
pub use pallas_addresses::{
    ByronAddress, Error as AddressError, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart,
    StakeAddress, StakePayload,
    byron::{AddrAttrProperty, AddrType, AddressPayload},
};

// TODO: Temporary re-exports until Pallas migrations
//
// See above.
pub use pallas_primitives::conway::{Constr, KeepRaw, MaybeIndefArray};

// TODO: Temporary re-exports until Pallas migrations
//
// See above.
pub use pallas_traverse::{ComputeHash, OriginalHash};

pub mod cardano;
pub use cardano::{
    account::Account,
    address::{Address, is_locked_by_script},
    anchor::Anchor,
    asset_name::AssetName,
    auxiliary_data::AuxiliaryData,
    ballot::Ballot,
    ballot_id::BallotId,
    bigint::BigInt,
    block::Block,
    block_header::BlockHeader,
    block_height::BlockHeight,
    bootstrap_witness::BootstrapWitness,
    bytes::Bytes,
    certificate::Certificate,
    certificate_pointer::CertificatePointer,
    constitution::Constitution,
    constitutional_committee::ConstitutionalCommitteeStatus,
    cost_model::CostModel,
    cost_models::CostModels,
    drep::DRep,
    drep_registration::DRepRegistration,
    drep_state::DRepState,
    drep_voting_thresholds::DRepVotingThresholds,
    epoch::Epoch,
    era_history::{
        EraHistory, EraHistoryError, EraHistoryFileError, MAINNET_ERA_HISTORY, PREPROD_ERA_HISTORY,
        PREVIEW_ERA_HISTORY, TESTNET_ERA_HISTORY, load_era_history_from_file,
    },
    era_name::{EraName, EraNameError},
    era_params::EraParams,
    ex_units::{ExUnits, sum_ex_units},
    ex_units_prices::ExUnitPrices,
    governance_action::GovernanceAction,
    hash::{
        Hash, Hasher, HeaderHash, NULL_HASH28, NULL_HASH32, ORIGIN_HASH, PoolId, TransactionId,
        size,
    },
    header::Header,
    header_body::HeaderBody,
    int::Int,
    language::Language,
    lovelace::Lovelace,
    memoized::{
        MemoizedDatum, MemoizedNativeScript, MemoizedPlutusData, MemoizedScript,
        MemoizedTransactionOutput, decode_script, deserialize_script, encode_script,
        from_minted_script, script_original_bytes, serialize_memoized_script, serialize_script,
    },
    metadatum::Metadatum,
    native_script::NativeScript,
    network::Network,
    network_id::NetworkId,
    network_magic::NetworkMagic,
    network_name::NetworkName,
    non_zero_int::NonZeroInt,
    nonce::{Nonce, parse_nonce},
    ordered_redeemer::OrderedRedeemer,
    peer::Peer,
    plutus_data::PlutusData,
    plutus_script::PlutusScript,
    point::Point,
    pool_metadata::PoolMetadata,
    pool_params::PoolParams,
    pool_voting_thresholds::PoolVotingThresholds,
    positive_coin::PositiveCoin,
    proposal::Proposal,
    proposal_id::{ComparableProposalId, ProposalId},
    proposal_pointer::ProposalPointer,
    proposal_state::ProposalState,
    protocol_parameters::{
        ConsensusParameters, GlobalParameters, MAINNET_GLOBAL_PARAMETERS,
        PREPROD_GLOBAL_PARAMETERS, PREPROD_INITIAL_PROTOCOL_PARAMETERS, PREVIEW_GLOBAL_PARAMETERS,
        PREVIEW_INITIAL_PROTOCOL_PARAMETERS, ProtocolParameters, TESTNET_GLOBAL_PARAMETERS,
    },
    protocol_parameters_update::{ProtocolParamUpdate, display_protocol_parameters_update},
    protocol_version::{PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, ProtocolVersion},
    rational_number::RationalNumber,
    raw_block::RawBlock,
    redeemer::Redeemer,
    redeemer_key::RedeemerKey,
    redeemers::Redeemers,
    relay::Relay,
    required_script::RequiredScript,
    reward::Reward,
    reward_account::{
        RewardAccount, expect_stake_credential, new_stake_address,
        reward_account_to_stake_credential,
    },
    reward_kind::RewardKind,
    script_kind::ScriptKind,
    script_purpose::{ScriptPurpose, script_purpose_to_string},
    slot::{Slot, SlotArithmeticError},
    stake_credential::{
        BorrowedStakeCredential, StakeCredential, stake_credential_from_reward_account,
    },
    stake_credential_kind::StakeCredentialKind,
    time_ms::TimeMs,
    tip::Tip,
    transaction::Transaction,
    transaction_body::TransactionBody,
    transaction_input::{TransactionInput, transaction_input_to_string},
    transaction_pointer::TransactionPointer,
    value::Value,
    vkey_witness::verify_ed25519_signature,
    vkey_witness::{InvalidEd25519Signature, VKeyWitness},
    vote::Vote,
    voter::Voter,
    voter_kind::VoterKind,
    voting_procedure::VotingProcedure,
    witness_set::WitnessSet,
};
#[cfg(any(test, feature = "test-utils"))]
pub use cardano::{
    address::any_shelley_address,
    anchor::any_anchor,
    ballot::any_ballot,
    ballot_id::{any_ballot_id, any_voter},
    block_header::{
        any_fake_header, any_header, any_header_hash, any_header_with_parent,
        any_header_with_some_parent, any_headers_chain, any_headers_chain_with_root, make_header,
    },
    block_height::any_block_height,
    certificate_pointer::any_certificate_pointer,
    constitution::any_constitution,
    constitutional_committee::any_constitutional_committee_status,
    drep::any_drep,
    epoch::any_epoch,
    era_name::any_era_name,
    hash::{any_hash28, any_hash32},
    network::any_network,
    network_magic::any_network_magic,
    network_name::any_network_name,
    point::{any_point, any_specific_point},
    pool_params::any_pool_params,
    proposal::any_proposal,
    proposal_id::{any_comparable_proposal_id, any_proposal_id},
    proposal_pointer::any_proposal_pointer,
    protocol_parameters::{
        any_cost_model, any_cost_models, any_drep_voting_thresholds, any_ex_unit_prices,
        any_ex_units, any_ex_units_prices, any_gov_action, any_guardrails_script,
        any_pool_voting_thresholds, any_protocol_parameter, any_protocol_params_update,
        any_protocol_version, any_withdrawal,
    },
    rational_number::any_rational_number,
    reward_account::any_reward_account,
    stake_credential::any_stake_credential,
    tip::any_tip,
    transaction_pointer::any_transaction_pointer,
    vote::{VOTE_ABSTAIN, VOTE_NO, VOTE_YES, any_vote, any_vote_ref},
};

pub mod cbor {
    pub use amaru_minicbor_extra::{
        TAG_MAP_259, TAG_SET_258, allow_tag, check_tagged_array_length, decode_break, from_cbor,
        from_cbor_no_leftovers, heterogeneous_array, heterogeneous_map, lazy, missing_field, tee,
        to_cbor, unexpected_field,
    };
    pub use minicbor::{
        CborLen, Decode, Decoder, Encode, Encoder, bytes,
        data::{self, IanaTag, Tag, Type},
        decode, decode_with, display, encode, encode_with, len, len_with, to_vec, to_vec_with,
    };
    pub use pallas_codec::utils::AnyCbor as Any;
}
pub use cbor::{from_cbor, from_cbor_no_leftovers, to_cbor};

mod data_structures;
#[cfg(any(test, feature = "test-utils"))]
pub use data_structures::nullable::any_nullable;
#[doc(hidden)]
pub use data_structures::{
    ignore_eq::IgnoreEq,
    key_value_pairs::{IntoKeyValuePairsError, KeyValuePairs},
    legacy::Legacy,
    non_empty_bytes::{EmptyBytesError, NonEmptyBytes},
    non_empty_key_value_pairs::{IntoNonEmptyKeyValuePairsError, NonEmptyKeyValuePairs},
    non_empty_set::{IntoNonEmptySetError, NonEmptySet},
    non_empty_vec::{IntoNonEmptyVecError, NonEmptyVec},
    nullable::Nullable,
    set::Set,
    strict_maybe::StrictMaybe,
};

pub use serde_json as json;

pub mod macros;

mod traits;
#[doc(hidden)]
pub use traits::{
    AsHash, AsIndex, AsShelley, HasExUnits, HasLovelace, HasNetwork, HasOwnership, HasRedeemers,
    HasScriptHash, IsHeader, as_hash, as_index, as_shelley, has_ex_units, has_lovelace,
    has_network, has_ownership, has_redeemers, has_script_hash, is_header,
};

pub mod utils;
