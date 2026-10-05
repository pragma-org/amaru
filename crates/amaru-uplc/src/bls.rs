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

use bumpalo::collections::Vec as BumpVec;
use once_cell::sync::Lazy;

use crate::{
    arena::Arena,
    constant::{Integer, integer_from_bytes},
};

pub static SCALAR_PERIOD: Lazy<Integer> = Lazy::new(|| {
    let bytes: [u8; 32] = [
        0x73, 0xed, 0xa7, 0x53, 0x29, 0x9d, 0x7d, 0x48, 0x33, 0x39, 0xd8, 0x08, 0x09, 0xa1, 0xd8, 0x05, 0x53, 0xbd,
        0xa4, 0x02, 0xff, 0xfe, 0x5b, 0xfe, 0xff, 0xff, 0xff, 0xff, 0x00, 0x00, 0x00, 0x01,
    ];

    integer_from_bytes(&bytes, true)
});

pub const BLST_P1_COMPRESSED_SIZE: usize = 48;

pub const BLST_P2_COMPRESSED_SIZE: usize = 96;

pub const INTEGER_TO_BYTE_STRING_MAXIMUM_OUTPUT_LENGTH: i64 = 8192;

#[derive(Debug, thiserror::Error)]
#[error("BLS error: {0:?}")]
pub struct BlsError(blst::BLST_ERROR);

pub trait Compressable {
    fn compress<'a>(&self, arena: &'a Arena) -> &'a [u8];

    fn uncompress<'a>(arena: &'a Arena, bytes: &[u8]) -> Result<&'a Self, BlsError>
    where
        Self: std::marker::Sized;
}

impl Compressable for blst::blst_p1 {
    fn compress<'a>(&self, arena: &'a Arena) -> &'a [u8] {
        let mut out = [0u8; BLST_P1_COMPRESSED_SIZE];

        unsafe {
            blst::blst_p1_compress(&mut out as *mut _, self);
        };

        arena.alloc(BumpVec::from_iter_in(out, arena.as_bump()))
    }

    fn uncompress<'a>(arena: &'a Arena, bytes: &[u8]) -> Result<&'a Self, BlsError> {
        if bytes.len() != BLST_P1_COMPRESSED_SIZE {
            return Err(BlsError(blst::BLST_ERROR::BLST_BAD_ENCODING));
        }

        let mut affine = blst::blst_p1_affine::default();

        let out = arena.alloc(blst::blst_p1::default());

        unsafe {
            let err = blst::blst_p1_uncompress(&mut affine as *mut _, bytes.as_ptr());

            if err != blst::BLST_ERROR::BLST_SUCCESS {
                return Err(BlsError(err));
            }

            blst::blst_p1_from_affine(out as *mut _, &affine);

            let in_group = blst::blst_p1_in_g1(out);

            if !in_group {
                return Err(BlsError(blst::BLST_ERROR::BLST_POINT_NOT_IN_GROUP));
            }
        };

        Ok(out)
    }
}

impl Compressable for blst::blst_p2 {
    fn compress<'a>(&self, arena: &'a Arena) -> &'a [u8] {
        let mut out = [0; BLST_P2_COMPRESSED_SIZE];

        unsafe {
            blst::blst_p2_compress(&mut out as *mut _, self);
        };

        arena.alloc(BumpVec::from_iter_in(out, arena.as_bump()))
    }

    fn uncompress<'a>(arena: &'a Arena, bytes: &[u8]) -> Result<&'a Self, BlsError> {
        if bytes.len() != BLST_P2_COMPRESSED_SIZE {
            return Err(BlsError(blst::BLST_ERROR::BLST_BAD_ENCODING));
        }

        let mut affine = blst::blst_p2_affine::default();

        let out = arena.alloc(blst::blst_p2::default());

        unsafe {
            let err = blst::blst_p2_uncompress(&mut affine as *mut _, bytes.as_ptr());

            if err != blst::BLST_ERROR::BLST_SUCCESS {
                return Err(BlsError(err));
            }

            blst::blst_p2_from_affine(out as *mut _, &affine);

            let in_group = blst::blst_p2_in_g2(out);

            if !in_group {
                return Err(BlsError(blst::BLST_ERROR::BLST_POINT_NOT_IN_GROUP));
            }
        };

        Ok(out)
    }
}
