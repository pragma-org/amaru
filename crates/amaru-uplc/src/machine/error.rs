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

use std::array::TryFromSliceError;

use super::{ExBudget, MachineVersion, value::Value};
use crate::{
    binder::Eval,
    bls::BlsError,
    builtin::DefaultFunction,
    constant::{Constant, Integer},
    data::PlutusData,
    ledger_value::ValueError,
    term::Term,
    typ::Type,
};

#[derive(thiserror::Error, Debug)]
pub enum MachineError<'a, V>
where
    V: Eval<'a>,
{
    #[error("Explicit error term")]
    ExplicitErrorTerm,
    #[error("Non-function application")]
    NonFunctionApplication(&'a Value<'a, V>, &'a Value<'a, V>),
    #[error("Non-constant value")]
    NotAConstant(&'a Value<'a, V>),
    #[error("Open term evaluated")]
    OpenTermEvaluated(&'a Term<'a, V>),
    #[error("Out of budget")]
    OutOfExError(ExBudget),
    #[error("Unexpected builtin term argument")]
    UnexpectedBuiltinTermArgument(&'a Term<'a, V>),
    #[error("Non-polymorphic instantiation")]
    NonPolymorphicInstantiation(&'a Value<'a, V>),
    #[error("Builtin term argument expected")]
    BuiltinTermArgumentExpected(&'a Term<'a, V>),
    #[error("Non-constructor scrutinized")]
    NonConstrScrutinized(&'a Value<'a, V>),
    #[error("Non-integer index")]
    MissingCaseBranch(&'a [&'a Term<'a, V>], &'a Value<'a, V>),
    #[error(transparent)]
    Runtime(RuntimeError<'a>),
    #[error("Max constr tag exceeded")]
    MaxConstrTagExceeded(&'a Value<'a, V>),
    #[error("No cost found for builtin function: {0:?}")]
    NoCostForBuiltin(DefaultFunction),
    #[error("Program version {}.{}.{} is not available", .0.major, .0.minor, .0.patch)]
    UnavailableProgramVersion(MachineVersion),
}

#[derive(thiserror::Error, Debug)]
pub enum RuntimeError<'a> {
    #[error("Byte string out of bounds")]
    ByteStringOutOfBounds(&'a [u8], &'a Integer),
    #[error("Type mismatch")]
    TypeMismatch(Type<'a>, &'a Constant<'a>),
    #[error("Expected pair")]
    ExpectedPair(&'a Constant<'a>),
    #[error("Expected list")]
    ExpectedList(&'a Constant<'a>),
    #[error("Expected array")]
    ExpectedArray(&'a Constant<'a>),
    #[error("Not data")]
    NotData(&'a Constant<'a>),
    #[error("Malformed data")]
    MalFormedData(&'a PlutusData<'a>),
    #[error("Empty list")]
    EmptyList(&'a [&'a Constant<'a>]),
    #[error("Unexpected Ed25519 public key length")]
    UnexpectedEd25519PublicKeyLength(TryFromSliceError),
    #[error("Unexpected Ed25519 signature length")]
    UnexpectedEd25519SignatureLength(TryFromSliceError),
    #[error("Division by zero")]
    DivisionByZero(&'a Integer, &'a Integer),
    #[error("Integer out of bounds")]
    IntegerOutOfBounds(&'a Integer),
    #[error("MkCons type mismatch")]
    MkConsTypeMismatch(&'a Constant<'a>),
    #[error("Byte string cons not a byte")]
    ByteStringConsNotAByte(&'a Integer),
    #[error("constrData: tag {0} is not within the bounds of a Word64")]
    ConstrTagOutOfBounds(&'a Integer),
    #[error(transparent)]
    Secp256k1(#[from] secp256k1::Error),
    #[error(transparent)]
    DecodeUtf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    Bls(#[from] BlsError),
    #[error("Bls Error: Hash to curve dst too big")]
    HashToCurveDstTooBig,
    #[error("bytes size beyond limit when converting from integer\n         Size {0}\n      Maximum {1}")]
    IntegerToByteStringSizeTooBig(&'a Integer, i64),
    #[error("bytes size below limit when converting from integer\n         Size {0}\n      Minimum {1}")]
    IntegerToByteStringSizeTooSmall(&'a Integer, usize),
    #[error("integerToByteString encountered negative input\n        Input {0}")]
    IntegerToByteStringNegativeInput(&'a Integer),
    #[error("integerToByteString encountered negative size\n         Size {0}")]
    IntegerToByteStringNegativeSize(&'a Integer),
    #[error("Empty byte array")]
    EmptyByteArray,
    #[error("readBit: index out of bounds\n        Index {0}\n         Size {1}")]
    ReadBitOutOfBounds(&'a Integer, usize),
    #[error("writeBits: an index is out of bounds\n        Index {0}\n         Size {1}")]
    WriteBitsOutOfBounds(&'a Integer, usize),
    #[error("writeBits: input too long\n       Length {0}\n      Maximum {1}")]
    WriteBitsInputTooLong(usize, usize),
    #[error("{0} is not within the bounds of a Byte")]
    OutsideByteBounds(&'a Integer),
    #[error("{0} is not within the bounds of usize")]
    OutsideUsizeBounds(&'a Integer),
    #[error("bytes size beyond limit when replicating byte\n         Size {0}\n      Maximum {1}")]
    ReplicateByteSizeTooBig(&'a Integer, i64),
    #[error("bytes size below limit when replicating byte\n         Size {0}\n      Minimum {1}")]
    ReplicateByteSizeTooSmall(&'a Integer, usize),
    #[error("replicateByte encountered negative input\n        Input {0}")]
    ReplicateByteNegativeInput(&'a Integer),
    #[error("replicateByte encountered negative size\n         Size {0}")]
    ReplicateByteNegativeSize(&'a Integer),
    #[error("indexArray: index out of bounds\n        Index {0}\n         Size {1}")]
    IndexArrayOutOfBounds(&'a Integer, usize),
    #[error("Serialization error")]
    SerializationError(&'a PlutusData<'a>),
    #[error("Scalar exceeds 512-byte bound for multiScalarMul")]
    MultiScalarMulScalarOutOfBounds,
    #[error(transparent)]
    Value(#[from] ValueError),
}

impl<'a, V> MachineError<'a, V>
where
    V: Eval<'a>,
{
    pub fn runtime(runtime_error: RuntimeError<'a>) -> Self {
        MachineError::Runtime(runtime_error)
    }

    pub fn type_mismatch(expected: Type<'a>, constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::TypeMismatch(expected, constant))
    }

    pub fn mk_cons_type_mismatch(constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::MkConsTypeMismatch(constant))
    }

    pub fn expected_pair(constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::ExpectedPair(constant))
    }

    pub fn expected_list(constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::ExpectedList(constant))
    }

    pub fn expected_array(constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::ExpectedArray(constant))
    }

    pub fn empty_list(constant: &'a [&'a Constant<'a>]) -> Self {
        MachineError::runtime(RuntimeError::EmptyList(constant))
    }

    pub fn byte_string_out_of_bounds(byte_string: &'a [u8], index: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::ByteStringOutOfBounds(byte_string, index))
    }

    pub fn not_data(constant: &'a Constant<'a>) -> Self {
        MachineError::runtime(RuntimeError::NotData(constant))
    }

    pub fn malformed_data(plutus_data: &'a PlutusData<'a>) -> Self {
        MachineError::runtime(RuntimeError::MalFormedData(plutus_data))
    }

    pub fn unexpected_ed25519_public_key_length(length: TryFromSliceError) -> Self {
        MachineError::runtime(RuntimeError::UnexpectedEd25519PublicKeyLength(length))
    }

    pub fn unexpected_ed25519_signature_length(length: TryFromSliceError) -> Self {
        MachineError::runtime(RuntimeError::UnexpectedEd25519SignatureLength(length))
    }

    pub fn division_by_zero(numerator: &'a Integer, denominator: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::DivisionByZero(numerator, denominator))
    }

    pub fn integer_out_of_bounds(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::IntegerOutOfBounds(integer))
    }

    pub fn byte_string_cons_not_a_byte(byte: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::ByteStringConsNotAByte(byte))
    }

    pub fn constr_tag_out_of_bounds(tag: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::ConstrTagOutOfBounds(tag))
    }

    pub fn secp256k1(error: secp256k1::Error) -> Self {
        MachineError::runtime(RuntimeError::Secp256k1(error))
    }

    pub fn decode_utf8(error: std::str::Utf8Error) -> Self {
        MachineError::runtime(RuntimeError::DecodeUtf8(error))
    }

    pub fn bls(error: BlsError) -> Self {
        MachineError::runtime(RuntimeError::Bls(error))
    }

    pub fn hash_to_curve_dst_too_big() -> Self {
        MachineError::runtime(RuntimeError::HashToCurveDstTooBig)
    }

    pub fn integer_to_byte_string_size_too_big(integer: &'a Integer, maximum: i64) -> Self {
        MachineError::runtime(RuntimeError::IntegerToByteStringSizeTooBig(integer, maximum))
    }

    pub fn integer_to_byte_string_size_too_small(integer: &'a Integer, minimum: usize) -> Self {
        MachineError::runtime(RuntimeError::IntegerToByteStringSizeTooSmall(integer, minimum))
    }

    pub fn integer_to_byte_string_negative_input(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::IntegerToByteStringNegativeInput(integer))
    }

    pub fn integer_to_byte_string_negative_size(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::IntegerToByteStringNegativeSize(integer))
    }

    pub fn empty_byte_array() -> Self {
        MachineError::runtime(RuntimeError::EmptyByteArray)
    }

    pub fn read_bit_out_of_bounds(index: &'a Integer, size: usize) -> Self {
        MachineError::runtime(RuntimeError::ReadBitOutOfBounds(index, size))
    }

    pub fn write_bits_out_of_bounds(index: &'a Integer, size: usize) -> Self {
        MachineError::runtime(RuntimeError::WriteBitsOutOfBounds(index, size))
    }

    pub fn write_bits_input_too_long(length: usize, maximum: usize) -> Self {
        MachineError::runtime(RuntimeError::WriteBitsInputTooLong(length, maximum))
    }

    pub fn outside_byte_bounds(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::OutsideByteBounds(integer))
    }

    pub fn outside_usize_bounds(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::OutsideUsizeBounds(integer))
    }

    pub fn replicate_byte_negative_size(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::ReplicateByteNegativeSize(integer))
    }

    pub fn replicate_byte_size_too_big(integer: &'a Integer, maximum: i64) -> Self {
        MachineError::runtime(RuntimeError::ReplicateByteSizeTooBig(integer, maximum))
    }

    pub fn replicate_byte_negative_input(integer: &'a Integer) -> Self {
        MachineError::runtime(RuntimeError::ReplicateByteNegativeInput(integer))
    }

    pub fn index_array_out_of_bounds(index: &'a Integer, size: usize) -> Self {
        MachineError::runtime(RuntimeError::IndexArrayOutOfBounds(index, size))
    }

    pub fn serialization_error(data: &'a PlutusData<'a>) -> Self {
        MachineError::runtime(RuntimeError::SerializationError(data))
    }
}
