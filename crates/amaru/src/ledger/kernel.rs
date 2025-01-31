// This module acts as an anti-corruption layer between the amaru codebase and pallas. Said
// differently, it mostly re-exports primitives and types from pallas, in a way that prevent pallas
// imports from spreading across the rest of the ledger sub-component.
//
// It's also the right place to put rather general functions or types that ought to be in pallas.
// While elements are being contributed upstream, they might transiently live in this module.

use std::sync::LazyLock;

use num::{rational::Ratio, BigUint};
pub use ouroboros::ledger::PoolSigma;
use pallas_addresses::*;
pub use pallas_codec::{
    minicbor as cbor,
    utils::{NonEmptyKeyValuePairs, Nullable, Set},
};
pub use pallas_crypto::hash::{Hash, Hasher};
pub use pallas_primitives::{
    alonzo,
    conway::{
        AddrKeyhash, Certificate, Coin, DRep, Epoch, MintedBlock, MintedTransactionBody,
        MintedTransactionOutput, PoolMetadata, RationalNumber, Relay, RewardAccount,
        StakeCredential, TransactionInput, TransactionOutput, UnitInterval, Value, VrfKeyhash,
    },
};

// Constants
// ----------------------------------------------------------------------------

/// Maximum supply of Ada, in lovelace (1 Ada = 1,000,000 Lovelace)
pub const MAX_LOVELACE_SUPPLY: u64 = 45000000000000000;

/// The maximum depth of a rollback, also known as the security parameter 'k'.
/// This translates down to the length of our volatile storage, containing states of the ledger
/// which aren't yet considered final.
///
// FIXME: import from genesis configuration
pub const CONSENSUS_SECURITY_PARAM: usize = 2160;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Shelley's epoch length.
pub const SHELLEY_EPOCH_LENGTH_SCALE_FACTOR: usize = 10;

/// Inverse of the active slot coefficient (i.e. 1/f);
pub const ACTIVE_SLOT_COEFF_INVERSE: usize = 20;

/// Number of slots in a Shelley epoch
pub const SHELLEY_EPOCH_LENGTH: usize =
    ACTIVE_SLOT_COEFF_INVERSE * SHELLEY_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Relative slot from which data of the previous epoch can be considered stable.
pub const STABILITY_WINDOW: usize = ACTIVE_SLOT_COEFF_INVERSE * CONSENSUS_SECURITY_PARAM * 2;

/// Multiplier applied to the CONSENSUS_SECURITY_PARAM to determine Byron's epoch length.
pub const BYRON_EPOCH_LENGTH_SCALE_FACTOR: usize = 10;

/// Number of blocks in a Byron epoch
pub const BYRON_EPOCH_LENGTH: usize = BYRON_EPOCH_LENGTH_SCALE_FACTOR * CONSENSUS_SECURITY_PARAM;

/// Number of slots in the Byron era, for PreProd
pub const BYRON_TOTAL_SLOTS: usize = BYRON_EPOCH_LENGTH * PREPROD_SHELLEY_TRANSITION_EPOCH;

/// Epoch number in which the PreProd network transitioned to Shelley.
pub const PREPROD_SHELLEY_TRANSITION_EPOCH: usize = 4;

/// Value, in Lovelace, that one must deposit when registering a new stake pool
pub const STAKE_POOL_DEPOSIT: usize = 500000000;

/// Value, in Lovelace, that one must deposit when registering a new stake credential
pub const STAKE_CREDENTIAL_DEPOSIT: usize = 2000000;

// The monetary expansion value, a.k.a ρ
pub static MONETARY_EXPANSION: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(3_u64), BigUint::from(1000_u64)));

/// Treasury tax, a.k.a τ
pub static TREASURY_TAX: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(20_u64), BigUint::from(100_u64)));

/// Pledge influence parameter, a.k.a a0
pub static PLEDGE_INFLUENCE: LazyLock<Ratio<BigUint>> =
    LazyLock::new(|| Ratio::new_raw(BigUint::from(3_u64), BigUint::from(10_u64)));

/// The optimal number of stake pools target for the incentives, a.k.a k
pub const OPTIMAL_STAKE_POOLS_COUNT: usize = 500;

// Re-exports & extra aliases
// ----------------------------------------------------------------------------

// FIXME: Switch to BigInt or BigUInt eventually. Not doing it _now_ because of laziness, not
// having to deal with serialisation and deserialisation of those right now; and also because so
// far it isn't causing any problem.
pub type Lovelace = u64;

pub type Point = pallas_network::miniprotocols::Point;

pub type PoolId = Hash<28>;

pub type Slot = u64;

// PoolParams
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: VrfKeyhash,
    pub pledge: Coin,
    pub cost: Coin,
    pub margin: UnitInterval,
    pub reward_account: RewardAccount,
    pub owners: Set<AddrKeyhash>,
    pub relays: Vec<Relay>,
    pub metadata: Nullable<PoolMetadata>,
}

impl<C> cbor::encode::Encode<C> for PoolParams {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(9)?;
        e.encode_with(self.id, ctx)?;
        e.encode_with(self.vrf, ctx)?;
        e.encode_with(self.pledge, ctx)?;
        e.encode_with(self.cost, ctx)?;
        e.encode_with(&self.margin, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(&self.owners, ctx)?;
        e.encode_with(&self.relays, ctx)?;
        e.encode_with(&self.metadata, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(PoolParams {
            id: d.decode_with(ctx)?,
            vrf: d.decode_with(ctx)?,
            pledge: d.decode_with(ctx)?,
            cost: d.decode_with(ctx)?,
            margin: d.decode_with(ctx)?,
            reward_account: d.decode_with(ctx)?,
            owners: d.decode_with(ctx)?,
            relays: d.decode_with(ctx)?,
            metadata: d.decode_with(ctx)?,
        })
    }
}

impl serde::Serialize for PoolParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use pallas_addresses::Address;
        use serde::ser::SerializeStruct;
        use std::collections::BTreeMap;

        fn as_lovelace_map(n: u64) -> BTreeMap<String, BTreeMap<String, u64>> {
            let mut lovelace = BTreeMap::new();
            lovelace.insert("lovelace".to_string(), n);
            let mut ada = BTreeMap::new();
            ada.insert("ada".to_string(), lovelace);
            ada
        }

        fn as_string_ratio(r: &UnitInterval) -> String {
            format!("{}/{}", r.numerator, r.denominator)
        }

        fn as_bech32_addr(bytes: &[u8]) -> String {
            Address::from_bytes(bytes)
                .and_then(|addr| addr.to_bech32())
                .unwrap_or_else(|e| panic!("failed to encode address to bech32: {e:?}"))
        }

        struct WrapRelay<'a>(&'a Relay);

        impl serde::Serialize for WrapRelay<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self.0 {
                    Relay::SingleHostAddr(port, ipv4, ipv6) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostAddr", 4)?;
                        s.serialize_field("type", "ipAddress")?;
                        if let Nullable::Some(ipv4) = ipv4 {
                            s.serialize_field(
                                "ipv4",
                                &format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3]),
                            )?;
                        }
                        if let Nullable::Some(ipv6) = ipv6 {
                            s.serialize_field("ipv6", ipv6)?;
                        }
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::SingleHostName(port, hostname) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostName", 3)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::MultiHostName(hostname) => {
                        let mut s = serializer.serialize_struct("Relay::MultiHostName", 2)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        s.end()
                    }
                }
            }
        }

        let mut s = serializer.serialize_struct("PoolParams", 9)?;
        s.serialize_field("id", &hex::encode(self.id))?;
        s.serialize_field("vrfVerificationKeyHash", &hex::encode(self.vrf))?;
        s.serialize_field("pledge", &as_lovelace_map(self.pledge))?;
        s.serialize_field("cost", &as_lovelace_map(self.cost))?;
        s.serialize_field("margin", &as_string_ratio(&self.margin))?;
        s.serialize_field("rewardAccount", &as_bech32_addr(&self.reward_account))?;
        s.serialize_field(
            "owners",
            &self.owners.iter().map(hex::encode).collect::<Vec<String>>(),
        )?;
        s.serialize_field(
            "relays",
            &self
                .relays
                .iter()
                .map(WrapRelay)
                .collect::<Vec<WrapRelay<'_>>>(),
        )?;
        if let Nullable::Some(metadata) = &self.metadata {
            s.serialize_field("metadata", metadata)?;
        }
        s.end()
    }
}

// Helpers
// ----------------------------------------------------------------------------

pub fn encode_bech32(hrp: &str, payload: &[u8]) -> Result<String, bech32::EncodeError> {
    let hrp = bech32::Hrp::parse(hrp).unwrap_or_else(|e| panic!("invalid HRP: {e:?}"));
    bech32::encode::<bech32::Bech32>(hrp, payload)
}

/// Calculate the epoch number corresponding to a given slot on the PreProd network.
// FIXME: Design and implement a proper abstraction for slot arithmetic. See https://github.com/pragma-org/amaru/pull/26/files#r1807394364
pub fn epoch_from_slot(slot: u64) -> u64 {
    let shelley_slots = slot - BYRON_TOTAL_SLOTS as u64;
    (shelley_slots / SHELLEY_EPOCH_LENGTH as u64) + PREPROD_SHELLEY_TRANSITION_EPOCH as u64
}

/// Obtain the slot number relative to the epoch.
// FIXME: Design and implement a proper abstraction for slot arithmetic. See https://github.com/pragma-org/amaru/pull/26/files#r1807394364
pub fn relative_slot(slot: u64) -> u64 {
    let shelley_previous_slots = (epoch_from_slot(slot) - PREPROD_SHELLEY_TRANSITION_EPOCH as u64)
        * SHELLEY_EPOCH_LENGTH as u64;
    slot - shelley_previous_slots - BYRON_TOTAL_SLOTS as u64
}

/// FIXME: Ideally, we should either:
///
/// - Have pallas_traverse or a similar API works directly from base objects instead of KeepRaw
///   objects (i.e. MintedTransactionOutput)
/// - Ensure that our database iterator yields MintedTransactionOutput and not TransactionOutput, o
///   we can use pallas_traverse out of the box.
///
/// Doing the latter properly is a lifetime hell I am not willing to explore right now.
pub fn output_lovelace(output: &TransactionOutput) -> Lovelace {
    match output {
        TransactionOutput::Legacy(legacy) => match legacy.amount {
            alonzo::Value::Coin(lovelace) => lovelace,
            alonzo::Value::Multiasset(lovelace, _) => lovelace,
        },
        TransactionOutput::PostAlonzo(modern) => match modern.value {
            Value::Coin(lovelace) => lovelace,
            Value::Multiasset(lovelace, _) => lovelace,
        },
    }
}

/// FIXME: See 'output_lovelace', same remark applies.
pub fn output_stake_credential(output: &TransactionOutput) -> Option<StakeCredential> {
    let address = Address::from_bytes(match output {
        TransactionOutput::Legacy(legacy) => &legacy.address[..],
        TransactionOutput::PostAlonzo(modern) => &modern.address[..],
    })
    .unwrap_or_else(|_| panic!("unable to deserialise address from output: {output:#?}"));

    match address {
        Address::Shelley(shelley) => match shelley.delegation() {
            ShelleyDelegationPart::Key(key) => Some(StakeCredential::AddrKeyhash(*key)),
            ShelleyDelegationPart::Script(script) => Some(StakeCredential::ScriptHash(*script)),
            ShelleyDelegationPart::Pointer(..) | ShelleyDelegationPart::Null => None,
        },
        Address::Byron(..) => None,
        Address::Stake(..) => unreachable!("stake address inside output?"),
    }
}

// This function shouldn't exist and pallas should provide a RewardAccount = (Network,
// StakeCredential) out of the box instead of row bytes.
pub fn reward_account_to_stake_credential(account: &RewardAccount) -> Option<StakeCredential> {
    if let Ok(Address::Stake(stake_addr)) = Address::from_bytes(&account[..]) {
        match stake_addr.payload() {
            StakePayload::Stake(key) => Some(StakeCredential::AddrKeyhash(*key)),
            StakePayload::Script(script) => Some(StakeCredential::ScriptHash(*script)),
        }
    } else {
        None
    }
}

/// An 'unsafe' version of `reward_account_to_stake_credential` that panics when the given
/// RewardAccount isn't a `StakeCredential`.
pub fn expect_stake_credential(account: &RewardAccount) -> StakeCredential {
    reward_account_to_stake_credential(account)
        .unwrap_or_else(|| panic!("unexpected malformed reward account: {:?}", account))
}
