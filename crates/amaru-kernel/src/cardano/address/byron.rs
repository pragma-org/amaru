// Copyright 2026 PRAGMA
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

use std::ops::Deref;

use amaru_minicbor_extra::decode_bytes;
use sha3::{Digest, Sha3_256};

use crate::{BootstrapWitness, Hash, Hasher, Network, cbor};

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

// -------------------------------------------------------------------------------------------------
// ByronAddress
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub struct ByronAddress(AddressPayload);

impl Deref for ByronAddress {
    type Target = AddressPayload;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ByronAddress {
    pub fn from_bytes(value: &[u8]) -> Option<Self> {
        cbor::decode(value).ok()
    }

    /// Re-compute an address (verification key) root from a transaction witness.
    pub fn root(witness: &BootstrapWitness) -> Hash<28> {
        let mut xpub = [0u8; 64];
        xpub[..32].copy_from_slice(&witness.public_key[..]);
        xpub[32..].copy_from_slice(&witness.chain_code[..]);

        AddressPayload::root(
            AddressType::VerificationKey,
            &SpendingData::VerificationKey(xpub),
            witness.attributes.as_slice(),
        )
    }

    // Tries to decode an address from its hex representation
    pub fn from_base58(value: &str) -> Option<Self> {
        let bytes = base58::FromBase58::from_base58(value).ok()?;
        Self::from_bytes(&bytes)
    }

    /// Gets a numeric id describing the type of the address
    pub fn typeid(&self) -> u8 {
        0b1000
    }

    pub fn to_base58(&self) -> String {
        let bytes = self.to_vec();
        base58::ToBase58::to_base58(bytes.as_slice())
    }

    pub fn to_hex(&self) -> String {
        let bytes = self.to_vec();
        hex::encode(bytes)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        cbor::to_cbor(self)
    }

    /// According to the Byron address specification, the attributes can optionally contain a u32
    /// network discriminant, identifying a specific testnet network.
    ///
    /// When decoding Byron address attributes, the Haskell node defaults NetworkMagic to
    /// NetworkMainOrStage, unless otherwise specified. The discriminant can be any `NetworkMagic`
    /// (sometimes referred to as `ProtocolMagic`), identifying a specific testnet. If present, it is
    /// Testnet(discriminant).
    ///
    /// It does not, notabtly, validate this discriminant, as evidenced by this conflicting Byron address on Preprod:
    ///
    /// - `2cWKMJemoBaiqkR9D1YZ2xQ2BhVxzauukrsxm8ttZUrto1f7kr5J1tD9uhtEtTc9U4PuF` (found in tx `9738801cc4f7e46bb3561a138a403fa8470e8a4faf2df5009023e7bbcdf09cb4`)
    ///
    /// This address encodes a `NetworkMagic` of `1097911063`. The `NetworkMagic` of Preprod is 1.
    ///
    /// As a result, since we are only checking the network of a Byron address for validation, we will
    /// mirror the Haskell node logic and disregard the discriminant when fetching the network from an
    /// address.
    ///
    /// Sources:
    ///
    /// - <https://raw.githubusercontent.com/cardano-foundation/CIPs/master/CIP-0019/CIP-0019-byron-addresses.cddl>
    /// - <https://book.world.dev.cardano.org/environments/preprod/byron-genesis.json>
    /// - <https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/eras/byron/ledger/impl/src/Cardano/Chain/Common/AddrAttributes.hs#L122-L144>
    /// - <https://github.com/IntersectMBO/cardano-ledger/blob/2d1e94cf96d00ba0da53883c388fa0aba6d74624/libs/cardano-ledger-core/src/Cardano/Ledger/Address.hs#L152>
    pub fn network(&self) -> Network {
        for (_, attribute) in self.attributes.iter() {
            if let AddressAttribute::NetworkTag(_) = attribute {
                return Network::Testnet;
            }
        }

        Network::Mainnet
    }
}

impl<C> cbor::Encode<C> for ByronAddress {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.tag(cbor::IanaTag::Cbor)?;
        let payload = cbor::to_cbor(&self.0);
        let crc = CRC.checksum(&payload);
        e.bytes(&payload)?;
        e.u32(crc)?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for ByronAddress {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        if d.tag()? != cbor::IanaTag::Cbor.tag() {
            return Err(cbor::decode::Error::message("invalid tag for Byron address payload"));
        }

        let payload = decode_bytes(d)?.into_owned();
        let crc = d.u32()?;

        if CRC.checksum(&payload) != crc {
            return Err(cbor::decode::Error::message("invalid Byron address checksum"));
        }

        Ok(Self(
            cbor::from_cbor(&payload).ok_or_else(|| cbor::decode::Error::message("invalid Byron address payload"))?,
        ))
    }
}

// -------------------------------------------------------------------------------------------------
// AddressPayload
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub struct AddressPayload {
    pub root: Hash<28>,
    pub attributes: AddressAttributes,
    pub address_type: AddressType,
}

impl AddressPayload {
    pub fn new(address_type: AddressType, spending_data: &SpendingData, attributes: AddressAttributes) -> Self {
        let root = Self::root(address_type, spending_data, &cbor::to_cbor(&attributes));
        Self { root, attributes, address_type }
    }

    pub fn root(address_type: AddressType, spending_data: &SpendingData, raw_attributes: &[u8]) -> Hash<28> {
        let mut sha3 = Sha3_256::new();

        // This is fundamentally to_cbor((address_type, spending_data, attributes)); but with the
        // attributes pre-serialised.
        sha3.update([0x83]);
        sha3.update(cbor::to_cbor(&address_type));
        sha3.update(cbor::to_cbor(spending_data));
        sha3.update(raw_attributes);

        Hasher::<224>::hash(&sha3.finalize())
    }
}

impl<C> cbor::Encode<C> for AddressPayload {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;

        e.encode_with(self.root, ctx)?;

        e.encode_with(&self.attributes, ctx)?;

        e.encode_with(self.address_type, ctx)?;

        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for AddressPayload {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;

        let root = d.decode_with(ctx)?;
        let attributes = d.decode_with(ctx)?;
        let address_type = d.decode_with(ctx)?;

        Ok(Self { root, attributes, address_type })
    }
}

// -------------------------------------------------------------------------------------------------
// AddressAttributes
// -------------------------------------------------------------------------------------------------

// NOTE: Byron address' attributes serialisation.
//
// Not using a map here to keep attributes in decoding order for re-serialisation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub struct AddressAttributes(Vec<(u8, AddressAttribute)>);

impl Deref for AddressAttributes {
    type Target = Vec<(u8, AddressAttribute)>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<C> cbor::Encode<C> for AddressAttributes {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // FIXME: Worry about definite vs indefinite length here?
        e.map(self.0.len() as u64)?;

        for (k, v) in &self.0 {
            e.u8(*k)?;
            e.bytes(v.deref())?;
        }

        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for AddressAttributes {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let attributes = cbor::heterogeneous_map(
            d,
            Vec::new(),
            |d| d.u8(),
            |d, s, k| {
                match k {
                    1 => {
                        s.push((k, AddressAttribute::DerivationPath(decode_bytes(d)?.into_owned())));
                    }
                    2 => {
                        s.push((k, AddressAttribute::NetworkTag(decode_bytes(d)?.into_owned())));
                    }
                    _ => {
                        s.push((k, AddressAttribute::Unknown(decode_bytes(d)?.into_owned())));
                    }
                }
                Ok(())
            },
        )?;

        Ok(Self(attributes))
    }
}

// -------------------------------------------------------------------------------------------------
// AddressAttribute
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub enum AddressAttribute {
    DerivationPath(Vec<u8>),
    NetworkTag(Vec<u8>),
    Unknown(Vec<u8>),
}

impl Deref for AddressAttribute {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        match self {
            Self::DerivationPath(bytes) | Self::NetworkTag(bytes) | Self::Unknown(bytes) => bytes,
        }
    }
}

// -------------------------------------------------------------------------------------------------
// AddressType
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash, serde::Serialize, serde::Deserialize)]
pub enum AddressType {
    VerificationKey,
    RedemptionVoucher,
}

impl<C> cbor::Encode<C> for AddressType {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.u8(match self {
            Self::VerificationKey => 0,
            // 1 was for Script, never used, and eventually removed after the Byron reboot. No
            // longer parsable today.
            Self::RedemptionVoucher => 2,
        })?;

        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for AddressType {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.u8()? {
            0 => Ok(AddressType::VerificationKey),
            2 => Ok(AddressType::RedemptionVoucher),
            _ => Err(cbor::decode::Error::message("invalid legacy address type")),
        }
    }
}

// -------------------------------------------------------------------------------------------------
// SpendingData
// -------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd)]
pub enum SpendingData {
    /// A serialised (extended) Ed25519 key with its chain code (a.k.a XPub)
    VerificationKey([u8; 64]),
    /// A serialised Ed25519 key
    RedemptionVoucher([u8; 32]),
}

impl<'b, C> cbor::Decode<'b, C> for SpendingData {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        match d.u8()? {
            0 => Ok(Self::VerificationKey(*d.decode_with::<_, cbor::bytes::ByteArray<64>>(ctx)?.deref())),
            // 1 was for Script, never used, and eventually removed after the Byron reboot. No
            // longer parsable today.
            2 => Ok(Self::RedemptionVoucher(*d.decode_with::<_, cbor::bytes::ByteArray<32>>(ctx)?.deref())),
            _ => Err(cbor::decode::Error::message("unknown variant id for spending data")),
        }
    }
}

impl<C> cbor::Encode<C> for SpendingData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;

        match self {
            Self::VerificationKey(x) => {
                e.u8(0)?;
                e.bytes(x)?;
            }
            Self::RedemptionVoucher(x) => {
                e.u8(2)?;
                e.bytes(x)?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ByronAddress;
    use crate::cbor;

    const TEST_VECTORS: [&str; 3] = [
        "37btjrVyb4KDXBNC4haBVPCrro8AQPHwvCMp3RFhhSVWwfFmZ6wwzSK6JK1hY6wHNmtrpTf1kdbva8TCneM2YsiXT7mrzT21EacHnPpz5YyUdj64na",
        "DdzFFzCqrht7PQiAhzrn6rNNoADJieTWBt8KeK9BZdUsGyX9ooYD9NpMCTGjQoUKcHN47g8JMXhvKogsGpQHtiQ65fZwiypjrC6d3a4Q",
        "Ae2tdPwUPEZLs4HtbuNey7tK4hTKrwNwYtGqp7bDfCy2WdR3P6735W5Yfpe",
    ];

    #[test]
    fn roundtrip_base58() {
        for vector in TEST_VECTORS {
            let addr = ByronAddress::from_base58(vector).unwrap();
            let ours = addr.to_base58();
            assert_eq!(vector, ours);
        }
    }

    #[test]
    fn roundtrip_cbor() {
        for vector in TEST_VECTORS {
            let addr = ByronAddress::from_base58(vector).unwrap();
            let bytes = cbor::to_cbor(&addr);
            let addr = cbor::from_cbor::<ByronAddress>(&bytes).unwrap();
            let ours = addr.to_base58();
            assert_eq!(vector, ours);
        }
    }

    #[test]
    fn well_formed_envelope_with_invalid_payload() {
        let bytes = hex::decode("82D818582082581C8518129A3C0DF8E33C40E04B8D26AD3B0422D0FA9CA9255806A3F38B001AE781CD5B")
            .unwrap();
        assert!(dbg!(ByronAddress::from_bytes(&bytes)).is_none())
    }
}
