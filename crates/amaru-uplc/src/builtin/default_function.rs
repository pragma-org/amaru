// Copyright 2025 PRAGMA
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

use amaru_kernel::{ProtocolVersion, protocol_version};

#[repr(u8)]
#[allow(non_camel_case_types)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum DefaultFunction {
    // Integer functions
    AddInteger = 0,
    SubtractInteger = 1,
    MultiplyInteger = 2,
    DivideInteger = 3,
    QuotientInteger = 4,
    RemainderInteger = 5,
    ModInteger = 6,
    EqualsInteger = 7,
    LessThanInteger = 8,
    LessThanEqualsInteger = 9,
    // ByteString functions
    AppendByteString = 10,
    ConsByteString = 11,
    SliceByteString = 12,
    LengthOfByteString = 13,
    IndexByteString = 14,
    EqualsByteString = 15,
    LessThanByteString = 16,
    LessThanEqualsByteString = 17,
    // Cryptography and hash functions
    Sha2_256 = 18,
    Sha3_256 = 19,
    Blake2b_256 = 20,
    Keccak_256 = 71,
    Blake2b_224 = 72,
    VerifyEd25519Signature = 21,
    VerifyEcdsaSecp256k1Signature = 52,
    VerifySchnorrSecp256k1Signature = 53,
    // String functions
    AppendString = 22,
    EqualsString = 23,
    EncodeUtf8 = 24,
    DecodeUtf8 = 25,
    // Bool function
    IfThenElse = 26,
    // Unit function
    ChooseUnit = 27,
    // Tracing function
    Trace = 28,
    // Pairs functions
    FstPair = 29,
    SndPair = 30,
    // List functions
    ChooseList = 31,
    MkCons = 32,
    HeadList = 33,
    TailList = 34,
    NullList = 35,
    // Data functions
    // It is convenient to have a "choosing" function for a data type that has more than two
    // constructors to get pattern matching over it and we may end up having multiple such data
    // types, hence we include the name of the data type as a suffix.
    ChooseData = 36,
    ConstrData = 37,
    MapData = 38,
    ListData = 39,
    IData = 40,
    BData = 41,
    UnConstrData = 42,
    UnMapData = 43,
    UnListData = 44,
    UnIData = 45,
    UnBData = 46,
    EqualsData = 47,
    SerialiseData = 51,
    // Misc constructors
    // Constructors that we need for constructing e.g. Data. Polymorphic builtin
    // constructors are often problematic (See note [Representable built-in
    // functions over polymorphic built-in types])
    MkPairData = 48,
    MkNilData = 49,
    MkNilPairData = 50,

    // BLS Builtins
    Bls12_381_G1_Add = 54,
    Bls12_381_G1_Neg = 55,
    Bls12_381_G1_ScalarMul = 56,
    Bls12_381_G1_Equal = 57,
    Bls12_381_G1_Compress = 58,
    Bls12_381_G1_Uncompress = 59,
    Bls12_381_G1_HashToGroup = 60,
    Bls12_381_G2_Add = 61,
    Bls12_381_G2_Neg = 62,
    Bls12_381_G2_ScalarMul = 63,
    Bls12_381_G2_Equal = 64,
    Bls12_381_G2_Compress = 65,
    Bls12_381_G2_Uncompress = 66,
    Bls12_381_G2_HashToGroup = 67,
    Bls12_381_MillerLoop = 68,
    Bls12_381_MulMlResult = 69,
    Bls12_381_FinalVerify = 70,

    // Bitwise
    IntegerToByteString = 73,
    ByteStringToInteger = 74,

    AndByteString = 75,
    OrByteString = 76,
    XorByteString = 77,
    ComplementByteString = 78,
    ReadBit = 79,
    WriteBits = 80,
    ReplicateByte = 81,
    ShiftByteString = 82,
    RotateByteString = 83,
    CountSetBits = 84,
    FindFirstSetBit = 85,
    Ripemd_160 = 86,

    ExpModInteger = 87,
    DropList = 88,
    LengthOfArray = 89,
    ListToArray = 90,
    IndexArray = 91,

    // BLS Multi-Scalar Multiplication
    Bls12_381_G1_MultiScalarMul = 92,
    Bls12_381_G2_MultiScalarMul = 93,

    // Value builtins
    InsertCoin = 94,
    LookupCoin = 95,
    UnionValue = 96,
    ValueContains = 97,
    ValueData = 98,
    UnValueData = 99,
    ScaleValue = 100,
}

impl DefaultFunction {
    pub fn force_count(&self) -> usize {
        match self {
            DefaultFunction::AddInteger => 0,
            DefaultFunction::SubtractInteger => 0,
            DefaultFunction::MultiplyInteger => 0,
            DefaultFunction::DivideInteger => 0,
            DefaultFunction::QuotientInteger => 0,
            DefaultFunction::RemainderInteger => 0,
            DefaultFunction::ModInteger => 0,
            DefaultFunction::EqualsInteger => 0,
            DefaultFunction::LessThanInteger => 0,
            DefaultFunction::LessThanEqualsInteger => 0,
            DefaultFunction::AppendByteString => 0,
            DefaultFunction::ConsByteString => 0,
            DefaultFunction::SliceByteString => 0,
            DefaultFunction::LengthOfByteString => 0,
            DefaultFunction::IndexByteString => 0,
            DefaultFunction::EqualsByteString => 0,
            DefaultFunction::LessThanByteString => 0,
            DefaultFunction::LessThanEqualsByteString => 0,
            DefaultFunction::Sha2_256 => 0,
            DefaultFunction::Sha3_256 => 0,
            DefaultFunction::Blake2b_224 => 0,
            DefaultFunction::Blake2b_256 => 0,
            DefaultFunction::Keccak_256 => 0,
            DefaultFunction::VerifyEd25519Signature => 0,
            DefaultFunction::VerifyEcdsaSecp256k1Signature => 0,
            DefaultFunction::VerifySchnorrSecp256k1Signature => 0,
            DefaultFunction::AppendString => 0,
            DefaultFunction::EqualsString => 0,
            DefaultFunction::EncodeUtf8 => 0,
            DefaultFunction::DecodeUtf8 => 0,
            DefaultFunction::IfThenElse => 1,
            DefaultFunction::ChooseUnit => 1,
            DefaultFunction::Trace => 1,
            DefaultFunction::FstPair => 2,
            DefaultFunction::SndPair => 2,
            DefaultFunction::ChooseList => 2,
            DefaultFunction::MkCons => 1,
            DefaultFunction::HeadList => 1,
            DefaultFunction::TailList => 1,
            DefaultFunction::NullList => 1,
            DefaultFunction::ChooseData => 1,
            DefaultFunction::ConstrData => 0,
            DefaultFunction::MapData => 0,
            DefaultFunction::ListData => 0,
            DefaultFunction::IData => 0,
            DefaultFunction::BData => 0,
            DefaultFunction::UnConstrData => 0,
            DefaultFunction::UnMapData => 0,
            DefaultFunction::UnListData => 0,
            DefaultFunction::UnIData => 0,
            DefaultFunction::UnBData => 0,
            DefaultFunction::EqualsData => 0,
            DefaultFunction::SerialiseData => 0,
            DefaultFunction::MkPairData => 0,
            DefaultFunction::MkNilData => 0,
            DefaultFunction::MkNilPairData => 0,
            DefaultFunction::Bls12_381_G1_Add => 0,
            DefaultFunction::Bls12_381_G1_Neg => 0,
            DefaultFunction::Bls12_381_G1_ScalarMul => 0,
            DefaultFunction::Bls12_381_G1_Equal => 0,
            DefaultFunction::Bls12_381_G1_Compress => 0,
            DefaultFunction::Bls12_381_G1_Uncompress => 0,
            DefaultFunction::Bls12_381_G1_HashToGroup => 0,
            DefaultFunction::Bls12_381_G2_Add => 0,
            DefaultFunction::Bls12_381_G2_Neg => 0,
            DefaultFunction::Bls12_381_G2_ScalarMul => 0,
            DefaultFunction::Bls12_381_G2_Equal => 0,
            DefaultFunction::Bls12_381_G2_Compress => 0,
            DefaultFunction::Bls12_381_G2_Uncompress => 0,
            DefaultFunction::Bls12_381_G2_HashToGroup => 0,
            DefaultFunction::Bls12_381_MillerLoop => 0,
            DefaultFunction::Bls12_381_MulMlResult => 0,
            DefaultFunction::Bls12_381_FinalVerify => 0,
            DefaultFunction::IntegerToByteString => 0,
            DefaultFunction::ByteStringToInteger => 0,
            DefaultFunction::AndByteString => 0,
            DefaultFunction::OrByteString => 0,
            DefaultFunction::XorByteString => 0,
            DefaultFunction::ComplementByteString => 0,
            DefaultFunction::ReadBit => 0,
            DefaultFunction::WriteBits => 0,
            DefaultFunction::ReplicateByte => 0,
            DefaultFunction::ShiftByteString => 0,
            DefaultFunction::RotateByteString => 0,
            DefaultFunction::CountSetBits => 0,
            DefaultFunction::FindFirstSetBit => 0,
            DefaultFunction::Ripemd_160 => 0,
            DefaultFunction::ExpModInteger => 0,
            DefaultFunction::DropList => 1,
            DefaultFunction::LengthOfArray => 1,
            DefaultFunction::ListToArray => 1,
            DefaultFunction::IndexArray => 1,
            DefaultFunction::Bls12_381_G1_MultiScalarMul => 0,
            DefaultFunction::Bls12_381_G2_MultiScalarMul => 0,
            DefaultFunction::InsertCoin => 0,
            DefaultFunction::LookupCoin => 0,
            DefaultFunction::UnionValue => 0,
            DefaultFunction::ValueContains => 0,
            DefaultFunction::ValueData => 0,
            DefaultFunction::UnValueData => 0,
            DefaultFunction::ScaleValue => 0,
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            DefaultFunction::AddInteger => 2,
            DefaultFunction::SubtractInteger => 2,
            DefaultFunction::MultiplyInteger => 2,
            DefaultFunction::DivideInteger => 2,
            DefaultFunction::QuotientInteger => 2,
            DefaultFunction::RemainderInteger => 2,
            DefaultFunction::ModInteger => 2,
            DefaultFunction::EqualsInteger => 2,
            DefaultFunction::LessThanInteger => 2,
            DefaultFunction::LessThanEqualsInteger => 2,
            DefaultFunction::AppendByteString => 2,
            DefaultFunction::ConsByteString => 2,
            DefaultFunction::SliceByteString => 3,
            DefaultFunction::LengthOfByteString => 1,
            DefaultFunction::IndexByteString => 2,
            DefaultFunction::EqualsByteString => 2,
            DefaultFunction::LessThanByteString => 2,
            DefaultFunction::LessThanEqualsByteString => 2,
            DefaultFunction::Sha2_256 => 1,
            DefaultFunction::Sha3_256 => 1,
            DefaultFunction::Blake2b_224 => 1,
            DefaultFunction::Blake2b_256 => 1,
            DefaultFunction::Keccak_256 => 1,
            DefaultFunction::VerifyEd25519Signature => 3,
            DefaultFunction::VerifyEcdsaSecp256k1Signature => 3,
            DefaultFunction::VerifySchnorrSecp256k1Signature => 3,
            DefaultFunction::AppendString => 2,
            DefaultFunction::EqualsString => 2,
            DefaultFunction::EncodeUtf8 => 1,
            DefaultFunction::DecodeUtf8 => 1,
            DefaultFunction::IfThenElse => 3,
            DefaultFunction::ChooseUnit => 2,
            DefaultFunction::Trace => 2,
            DefaultFunction::FstPair => 1,
            DefaultFunction::SndPair => 1,
            DefaultFunction::ChooseList => 3,
            DefaultFunction::MkCons => 2,
            DefaultFunction::HeadList => 1,
            DefaultFunction::TailList => 1,
            DefaultFunction::NullList => 1,
            DefaultFunction::ChooseData => 6,
            DefaultFunction::ConstrData => 2,
            DefaultFunction::MapData => 1,
            DefaultFunction::ListData => 1,
            DefaultFunction::IData => 1,
            DefaultFunction::BData => 1,
            DefaultFunction::UnConstrData => 1,
            DefaultFunction::UnMapData => 1,
            DefaultFunction::UnListData => 1,
            DefaultFunction::UnIData => 1,
            DefaultFunction::UnBData => 1,
            DefaultFunction::EqualsData => 2,
            DefaultFunction::SerialiseData => 1,
            DefaultFunction::MkPairData => 2,
            DefaultFunction::MkNilData => 1,
            DefaultFunction::MkNilPairData => 1,
            DefaultFunction::Bls12_381_G1_Add => 2,
            DefaultFunction::Bls12_381_G1_Neg => 1,
            DefaultFunction::Bls12_381_G1_ScalarMul => 2,
            DefaultFunction::Bls12_381_G1_Equal => 2,
            DefaultFunction::Bls12_381_G1_Compress => 1,
            DefaultFunction::Bls12_381_G1_Uncompress => 1,
            DefaultFunction::Bls12_381_G1_HashToGroup => 2,
            DefaultFunction::Bls12_381_G2_Add => 2,
            DefaultFunction::Bls12_381_G2_Neg => 1,
            DefaultFunction::Bls12_381_G2_ScalarMul => 2,
            DefaultFunction::Bls12_381_G2_Equal => 2,
            DefaultFunction::Bls12_381_G2_Compress => 1,
            DefaultFunction::Bls12_381_G2_Uncompress => 1,
            DefaultFunction::Bls12_381_G2_HashToGroup => 2,
            DefaultFunction::Bls12_381_MillerLoop => 2,
            DefaultFunction::Bls12_381_MulMlResult => 2,
            DefaultFunction::Bls12_381_FinalVerify => 2,
            DefaultFunction::IntegerToByteString => 3,
            DefaultFunction::ByteStringToInteger => 2,
            DefaultFunction::AndByteString => 3,
            DefaultFunction::OrByteString => 3,
            DefaultFunction::XorByteString => 3,
            DefaultFunction::ComplementByteString => 1,
            DefaultFunction::ReadBit => 2,
            DefaultFunction::WriteBits => 3,
            DefaultFunction::ReplicateByte => 2,
            DefaultFunction::ShiftByteString => 2,
            DefaultFunction::RotateByteString => 2,
            DefaultFunction::CountSetBits => 1,
            DefaultFunction::FindFirstSetBit => 1,
            DefaultFunction::Ripemd_160 => 1,
            DefaultFunction::ExpModInteger => 3,
            DefaultFunction::DropList => 2,
            DefaultFunction::LengthOfArray => 1,
            DefaultFunction::ListToArray => 1,
            DefaultFunction::IndexArray => 2,
            DefaultFunction::Bls12_381_G1_MultiScalarMul => 2,
            DefaultFunction::Bls12_381_G2_MultiScalarMul => 2,
            DefaultFunction::InsertCoin => 4,
            DefaultFunction::LookupCoin => 3,
            DefaultFunction::UnionValue => 2,
            DefaultFunction::ValueContains => 2,
            DefaultFunction::ValueData => 1,
            DefaultFunction::UnValueData => 1,
            DefaultFunction::ScaleValue => 2,
        }
    }

    /// Check whether this builtin is available for a given Plutus ledger language
    /// and major protocol version.
    ///
    /// Follows the Haskell's `builtinsIntroducedIn` mapping from plutus-ledger-api:
    /// <https://github.com/IntersectMBO/plutus/blob/master/plutus-ledger-api/src/PlutusLedgerApi/Common/Versions.hs>
    pub fn is_available_in(&self, protocol_version: ProtocolVersion) -> bool {
        use DefaultFunction::*;

        if protocol_version >= protocol_version::PROTOCOL_VERSION_11 {
            true
        } else if protocol_version >= protocol_version::PROTOCOL_VERSION_10 {
            !matches!(
                self,
                ExpModInteger
                    | DropList
                    | LengthOfArray
                    | ListToArray
                    | IndexArray
                    | Bls12_381_G1_MultiScalarMul
                    | Bls12_381_G2_MultiScalarMul
                    | InsertCoin
                    | LookupCoin
                    | UnionValue
                    | ValueContains
                    | ValueData
                    | UnValueData
                    | ScaleValue
            )
        } else {
            unreachable!("unsupported protocol version: {protocol_version:?}")
        }
    }
}
