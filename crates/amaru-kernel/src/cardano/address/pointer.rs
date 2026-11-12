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

use std::io::Cursor;

/// An on-chain pointer to a stake credential
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, std::hash::Hash)]
pub struct AddressPointer {
    pub slot: u64,
    pub transaction: u64,
    pub certificate: u64,
}

impl AddressPointer {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let mut cursor = Cursor::new(bytes);

        let slot = uint7::read(&mut cursor)?;
        let transaction = uint7::read(&mut cursor)?;
        let certificate = uint7::read(&mut cursor)?;

        Some(Self { slot, transaction, certificate })
    }

    /// Parse a pointer the way the Haskell node does from protocol version 9 (`decodePtr`).
    ///
    /// The slot must fit in 32 bits and be written on at most 5 bytes, the transaction and certificate indexes must
    /// fit in 16 bits and be written on at most 3 bytes, and no byte may follow the certificate index.
    pub fn parse_strict(bytes: &[u8]) -> Option<Self> {
        let mut cursor = Cursor::new(bytes);

        let slot = uint7::read_bounded(&mut cursor, u32::MAX.into(), 5)?;
        let transaction = uint7::read_bounded(&mut cursor, u16::MAX.into(), 3)?;
        let certificate = uint7::read_bounded(&mut cursor, u16::MAX.into(), 3)?;

        (cursor.position() == bytes.len() as u64).then_some(Self { slot, transaction, certificate })
    }

    pub fn to_vec(&self) -> Vec<u8> {
        let mut cursor = Cursor::new(vec![]);

        uint7::write(&mut cursor, self.slot);
        uint7::write(&mut cursor, self.transaction);
        uint7::write(&mut cursor, self.certificate);

        cursor.into_inner()
    }
}

mod uint7 {
    use std::io::{Cursor, Read, Write};

    pub fn read(cursor: &mut Cursor<&[u8]>) -> Option<u64> {
        let mut output = 0u128;
        let mut buf = [0u8; 1];

        loop {
            cursor.read_exact(&mut buf).ok()?;

            let byte = buf[0];

            output = (output << 7) | (byte & 0x7F) as u128;

            if output > u64::MAX.into() {
                // NOTE: Overflows are clamped to u64::MAX
                return Some(u64::MAX);
            }

            if (byte & 0x80) == 0 {
                return Some(output as u64);
            }
        }
    }

    /// Read a value written on at most `max_len` bytes, and reject it when it is above `max`.
    ///
    /// `max_len` must be at most 9, so that the value fits in a `u64` while it is read.
    pub fn read_bounded(cursor: &mut Cursor<&[u8]>, max: u64, max_len: usize) -> Option<u64> {
        let mut output = 0u64;
        let mut buf = [0u8; 1];

        for _ in 0..max_len {
            cursor.read_exact(&mut buf).ok()?;

            let byte = buf[0];

            output = (output << 7) | (byte & 0x7F) as u64;

            if (byte & 0x80) == 0 {
                return (output <= max).then_some(output);
            }
        }

        None
    }

    pub fn write(cursor: &mut Cursor<Vec<u8>>, mut num: u64) {
        let mut output = vec![num as u8 & 0x7F];
        num /= 128;
        while num > 0 {
            output.push((num & 0x7F) as u8 | 0x80);
            num /= 128;
        }
        output.reverse();
        cursor.write_all(&output).unwrap_or_else(|_| unreachable!("cannot fail writing to a vector"));
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(&[0x00, 0x00, 0x00] => Some((0, 0, 0))                                            ; "zeros")]
    #[test_case(&[0x8f, 0xff, 0xff, 0xff, 0x7f, 0x83, 0xff, 0x7f, 0x83, 0xff, 0x7f]
        => Some((u32::MAX.into(), u16::MAX.into(), u16::MAX.into()))                               ; "largest values")]
    #[test_case(&[0x80, 0x01, 0x80, 0x02, 0x80, 0x03] => Some((1, 2, 3))                           ; "leading zero groups")]
    #[test_case(&[0x90, 0x80, 0x80, 0x80, 0x00, 0x00, 0x00] => None                                ; "slot above 32 bits")]
    #[test_case(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01, 0x00, 0x00] => None                          ; "slot on 6 bytes")]
    #[test_case(&[0x00, 0x84, 0x80, 0x00, 0x00] => None                                            ; "transaction index above 16 bits")]
    #[test_case(&[0x00, 0x80, 0x80, 0x80, 0x01, 0x00] => None                                      ; "transaction index on 4 bytes")]
    #[test_case(&[0x00, 0x00, 0x84, 0x80, 0x00] => None                                            ; "certificate index above 16 bits")]
    #[test_case(&[0x00, 0x00, 0x00, 0x00] => None                                                  ; "byte after the end")]
    #[test_case(&[0x00, 0x00, 0x80] => None                                                        ; "truncated")]
    fn parse_strict(bytes: &[u8]) -> Option<(u64, u64, u64)> {
        AddressPointer::parse_strict(bytes).map(|p| (p.slot, p.transaction, p.certificate))
    }
}
