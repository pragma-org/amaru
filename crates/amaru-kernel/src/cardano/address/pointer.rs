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
