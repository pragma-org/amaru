use crate::{
    Debug, KeepRaw, NonEmptySet,
    cbor::{Decode, Encode},
};
use serde::{Deserialize, Serialize};

pub use pallas_primitives::alonzo::BootstrapWitness;

#[derive(Serialize, Deserialize, Encode, Decode, Debug, PartialEq, Clone)]
#[cbor(map)]
pub struct WitnessSet {
    #[n(0)]
    pub vkeywitness: Option<NonEmptySet<pallas_primitives::conway::VKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptySet<pallas_primitives::conway::NativeScript>>,

    #[n(2)]
    pub bootstrap_witness: Option<NonEmptySet<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<1>>>,

    #[n(4)]
    pub plutus_data: Option<NonEmptySet<pallas_primitives::conway::PlutusData>>,

    #[n(5)]
    pub redeemer: Option<pallas_primitives::conway::Redeemers>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<3>>>,
}

#[derive(Encode, Decode, Debug, PartialEq, Clone)]
#[cbor(map)]
pub struct MintedWitnessSet<'b> {
    #[n(0)]
    pub vkeywitness: Option<NonEmptySet<pallas_primitives::conway::VKeyWitness>>,

    #[n(1)]
    pub native_script: Option<NonEmptySet<KeepRaw<'b, pallas_primitives::conway::NativeScript>>>,

    #[n(2)]
    pub bootstrap_witness: Option<NonEmptySet<BootstrapWitness>>,

    #[n(3)]
    pub plutus_v1_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<1>>>,

    #[b(4)]
    pub plutus_data: Option<NonEmptySet<KeepRaw<'b, pallas_primitives::conway::PlutusData>>>,

    #[n(5)]
    pub redeemer: Option<KeepRaw<'b, pallas_primitives::conway::Redeemers>>,

    #[n(6)]
    pub plutus_v2_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<2>>>,

    #[n(7)]
    pub plutus_v3_script: Option<NonEmptySet<pallas_primitives::conway::PlutusScript<3>>>,
}

impl<'b> From<MintedWitnessSet<'b>> for WitnessSet {
    fn from(x: MintedWitnessSet<'b>) -> Self {
        WitnessSet {
            vkeywitness: x.vkeywitness,
            native_script: x.native_script.map(Into::into),
            bootstrap_witness: x.bootstrap_witness,
            plutus_v1_script: x.plutus_v1_script,
            plutus_data: x.plutus_data.map(Into::into),
            redeemer: x.redeemer.map(|x| x.unwrap()),
            plutus_v2_script: x.plutus_v2_script,
            plutus_v3_script: x.plutus_v3_script,
        }
    }
}
