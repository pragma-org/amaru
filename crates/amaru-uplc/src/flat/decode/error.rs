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

use thiserror::Error;

#[derive(Error, Debug)]
pub enum FlatDecodeError {
    #[error("Reached end of buffer")]
    EndOfBuffer,
    #[error("Buffer is not byte aligned")]
    BufferNotByteAligned,
    #[error("Incorrect value of num_bits, must be less than 9")]
    IncorrectNumBits,
    #[error("Not enough data available, required {0} bytes")]
    NotEnoughBytes(usize),
    #[error("Not enough data available, required {0} bits")]
    NotEnoughBits(usize),
    #[error(transparent)]
    DecodeUtf8(#[from] std::str::Utf8Error),
    #[error(transparent)]
    DecodeCbor(#[from] minicbor::decode::Error),
    #[error("Decoding u32 to char {0}")]
    DecodeChar(u32),
    #[error("{0}")]
    Message(String),
    #[error("Default Function not found: {0}")]
    DefaultFunctionNotFound(u8),
    #[error("Unknown term constructor tag: {0}")]
    UnknownTermConstructor(u8),
    #[error("Unknown constant constructor tag: {0:#?}")]
    UnknownConstantConstructor(Vec<u8>),
    #[error("Unknown type tags: {0:#?}")]
    UnknownTypeTags(Vec<u8>),
    #[error("Missing type tag")]
    MissingTypeTag,
    #[error(
        "Flat decoding is not supported for BLS12-381 values; G1/G2 points are exchanged as bytestrings via compress/uncompress"
    )]
    BlsValueNotSupported,
    #[error("Trailing bytes after script: {0} bytes remaining")]
    TrailingBytes(usize),
    #[error("Builtin function {1} (tag {0}) is not available in the given language version")]
    BuiltinNotAvailable(u8, String),
    #[error("Constant type {1} (tag {0}) is not available before UPLC version 1.1.0")]
    ConstantTypeNotAvailable(u8, &'static str),
    #[error("Term {1} (tag {0}) is not available before UPLC version 1.1.0")]
    TermNotAvailable(u8, &'static str),
    #[error("Word value overflow: LEB128 value exceeds machine word size")]
    WordOverflow,
}
