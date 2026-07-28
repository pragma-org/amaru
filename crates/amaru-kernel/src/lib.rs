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
    ByronAddress, Error as AddressError, ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart, StakeAddress,
    StakePayload,
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

pub mod maths;

pub mod cardano;
pub use cardano::{
    account::Account,
    address::{Address, PlutusStakeAddress, is_locked_by_script},
    anchor::{self, Anchor},
    asset_name::AssetName,
    auxiliary_data::AuxiliaryData,
    ballot::Ballot,
    ballot_id::BallotId,
    bigint::BigInt,
    block::Block,
    block_header::BlockHeader,
    block_height::BlockHeight,
    bootstrap_witness::BootstrapWitness,
    bytes::{Bytes, empty_bytes},
    certificate::Certificate,
    certificate_pointer::CertificatePointer,
    consensus_parameters::ConsensusParameters,
    constitution::Constitution,
    constitutional_committee::ConstitutionalCommittee,
    constitutional_committee_member_status::ConstitutionalCommitteeMemberStatus,
    constitutional_committee_status::ConstitutionalCommitteeStatus,
    cost_model::{self, CostModel},
    cost_models::{self, CostModels},
    drep::{self, DRep},
    drep_registration::DRepRegistration,
    drep_state::DRepState,
    drep_voting_thresholds::{self, DRepVotingThresholds},
    epoch::Epoch,
    era_bound::EraBound,
    era_history::{
        EraHistory, EraHistoryError, EraHistoryFileError, MAINNET_ERA_HISTORY, PREPROD_ERA_HISTORY,
        PREVIEW_ERA_HISTORY, load_era_history_from_file,
    },
    era_name::{EraName, EraNameError},
    era_params::EraParams,
    era_summary::EraSummary,
    ex_units::{self, ExUnits, sum_ex_units},
    ex_units_prices::{self, ExUnitPrices},
    global_parameters::{
        GlobalParameters, MAINNET_GLOBAL_PARAMETERS, PREPROD_GLOBAL_PARAMETERS, PREVIEW_GLOBAL_PARAMETERS,
    },
    governance_action::GovernanceAction,
    hash::{self, Hash, Hasher, HeaderHash, NULL_HASH28, NULL_HASH32, ORIGIN_HASH, PoolId, size},
    header::Header,
    header_body::HeaderBody,
    int::Int,
    language::{self, Language},
    language_view::LanguageView,
    lovelace::Lovelace,
    memoized::{
        BorrowedScript, MemoizedDatum, MemoizedNativeScript, MemoizedPlutusData, MemoizedScript,
        MemoizedTransactionOutput, MemoizedValue, deserialize_script, serialize_memoized_script, serialize_script,
    },
    metadatum::Metadatum,
    multiasset::Multiasset,
    native_script::{NativeScript, evaluate_native_script},
    network::Network,
    network_id::NetworkId,
    network_magic::NetworkMagic,
    network_name::{NetworkName, PEER_SNAPSHOT_NETWORKS},
    non_zero_int::NonZeroInt,
    nonce::{Nonce, parse_nonce},
    operational_cert::OperationalCert,
    output_reference::OutputReference,
    peer::Peer,
    plutus_data::{PlutusData, PlutusDataSet, PlutusDatums},
    plutus_script::PlutusScript,
    plutus_version::{IsKnownPlutusVersion, KnownPlutusVersion, PlutusVersion, reify_plutus_version},
    point::Point,
    pool_metadata::{self, PoolMetadata},
    pool_params::PoolParams,
    pool_voting_thresholds::{self, PoolVotingThresholds},
    positive_coin::PositiveCoin,
    proposal::Proposal,
    proposal_id::{ComparableProposalId, ProposalId},
    proposal_pointer::ProposalPointer,
    proposal_state::ProposalState,
    proposals_roots::{self, ProposalsRoots, ProposalsRootsRc},
    protocol_parameters::{
        self, MAINNET_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
        PREVIEW_DEFAULT_PROTOCOL_PARAMETERS, ProtocolParameters,
    },
    protocol_parameters_update::{ProtocolParamUpdate, display_protocol_parameters_update},
    protocol_version::{self, PROTOCOL_VERSION_10, ProtocolVersion, ProtocolVersionTooOld},
    ratification_status::{self, RatificationStatus},
    rational_number::{self, RationalNumber},
    raw_block::RawBlock,
    redeemer::Redeemer,
    redeemer_key::RedeemerKey,
    redeemer_tag::{RedeemerTag, redeemer_tag_to_string},
    redeemers::{PallasRedeemers, PlutusRedeemers, RedeemerEntry, Redeemers},
    relay::{self, Relay},
    required_script::RequiredScript,
    required_signers::RequiredSigners,
    reward::Reward,
    reward_account::{
        PlutusWithdrawals, RewardAccount, WithdrawalError, expect_stake_credential, new_stake_address,
        reward_account_to_stake_credential,
    },
    reward_kind::RewardKind,
    script_context::ScriptContext,
    script_info::{ScriptInfo, ScriptPurpose},
    script_integrity_data::{ScriptIntegrityData, compute_script_integrity_hash},
    slot::{Slot, SlotArithmeticError},
    stake_credential::{BorrowedStakeCredential, StakeCredential, parse_reward_account},
    stake_credential_kind::StakeCredentialKind,
    time_range::TimeRange,
    tip::Tip,
    transaction::Transaction,
    transaction_body::TransactionBody,
    transaction_id::TransactionId,
    transaction_input::{TransactionInput, transaction_input_to_string},
    transaction_pointer::TransactionPointer,
    tx_info::{TxInfo, TxInfoTranslationError},
    utxos::Utxos,
    validity_interval::ValidityInterval,
    value::{CurrencySymbol, Mint, PlutusMint, Value},
    vkey_witness::{InvalidEd25519Signature, VKeyWitness, verify_ed25519_signature},
    vote::Vote,
    voter::Voter,
    voter_kind::VoterKind,
    voting_procedure::{PlutusVotes, VotingProcedure},
    vrf_cert::VrfCert,
    witness_set::WitnessSet,
};
#[cfg(any(test, feature = "test-utils"))]
pub use cardano::{
    address::any_shelley_address,
    anchor::any_anchor,
    ballot::any_ballot,
    ballot_id::{any_ballot_id, any_voter},
    block_header::{
        any_fake_header, any_header, any_header_hash, any_header_with_parent, any_header_with_some_parent,
        any_headers_chain, any_headers_chain_with_root, make_header, make_header_with_op_cert_seq,
    },
    block_height::any_block_height,
    certificate_pointer::any_certificate_pointer,
    constitution::any_constitution,
    constitutional_committee_status::any_constitutional_committee_status,
    drep::any_drep,
    drep_registration::any_drep_registration,
    epoch::any_epoch,
    era_bound::{any_era_bound, any_era_bound_for_epoch, any_era_bound_time},
    era_history::EraHistoryProxy,
    era_name::any_era_name,
    era_params::any_era_params,
    hash::{any_hash28, any_hash32},
    lovelace::any_lovelace,
    memoized::{any_datum, any_legacy_output, any_modern_output},
    network::any_network,
    network_magic::any_network_magic,
    network_name::any_network_name,
    point::{any_point, any_specific_point},
    pool_params::any_pool_params,
    proposal::any_proposal,
    proposal_id::{any_comparable_proposal_id, any_proposal_id},
    proposal_pointer::any_proposal_pointer,
    proposals_roots::any_proposals_roots,
    protocol_parameters::{
        any_cost_model, any_cost_models, any_drep_voting_thresholds, any_ex_unit_prices, any_ex_units,
        any_ex_units_prices, any_gov_action, any_guardrails_script, any_pool_voting_thresholds, any_protocol_parameter,
        any_protocol_params_update, any_protocol_version, any_withdrawal,
    },
    rational_number::any_rational_number,
    reward_account::any_reward_account,
    stake_credential::any_stake_credential,
    tip::any_tip,
    transaction_input::any_transaction_input,
    transaction_pointer::any_transaction_pointer,
    vote::{VOTE_ABSTAIN, VOTE_NO, VOTE_YES, any_vote, any_vote_ref},
};

pub mod cbor {
    pub use amaru_minicbor_extra::{
        TAG_MAP_259, TAG_SET_258, WithSize, allow_tag, check_tagged_array_length, collect_array_item_bytes,
        collect_map_value_bytes, decode_break, expect_tag, from_cbor, from_cbor_no_leftovers,
        from_cbor_no_leftovers_with, heterogeneous_array, heterogeneous_map, lazy, missing_field, tee, to_cbor,
        to_cbor_with, unexpected_field,
    };
    pub use minicbor::{
        CborLen, Decode, Decoder, Encode, Encoder, bytes,
        data::{self, IanaTag, Tag, Type},
        decode, decode_with, display, encode, encode_with, len, len_with, to_vec, to_vec_with,
    };
    pub use pallas_codec::utils::AnyCbor as Any;
}
pub use cbor::{from_cbor, from_cbor_no_leftovers, from_cbor_no_leftovers_with, to_cbor};

mod data_structures;
#[cfg(any(test, feature = "test-utils"))]
pub use data_structures::nullable::any_nullable;
pub use data_structures::{
    ignore_eq::IgnoreEq,
    key_value_pairs::{IntoKeyValuePairsError, KeyValuePairs, LegacyKeyValuePairs},
    legacy::Legacy,
    non_empty_bytes::{EmptyBytesError, NonEmptyBytes},
    non_empty_key_value_pairs::{IntoNonEmptyKeyValuePairsError, NonEmptyKeyValuePairs},
    non_empty_set::{IntoNonEmptySetError, NonEmptySet},
    non_empty_vec::{IntoNonEmptyVecError, NonEmptyVec},
    non_zero_duration::{NonZeroDuration, ZeroDurationError},
    nullable::Nullable,
    set::Set,
    strict_maybe::StrictMaybe,
};
pub use num;
pub use serde_json as json;

pub mod macros;

mod traits;
pub use traits::{
    AsHash, AsIndex, AsShelley, HasExUnits, HasLovelace, HasMajorVersion, HasNetwork, HasOwnership, HasRedeemers,
    HasScriptHash, HasTransactionId, IsHeader, ToBytes, as_hash, as_index, as_shelley, has_ex_units, has_lovelace,
    has_major_version, has_network, has_ownership, has_redeemers, has_script_hash, has_transaction_id, is_header,
    to_bytes,
};

pub mod utils;
