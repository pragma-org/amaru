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

use minicbor as cbor;
use std::{fs, io::Read};

/// A decoder that only consumes bytes CHUNK_SIZE at a time. Useful to decode large files while
/// maintaining memory usage low.
///
/// The decoder keeps an internal state with the bytes that have been read but not consumed, and a
/// handle to a source that implements [`std::io::Read`].
///
/// See [`Self::from_file`] for example, to lazily read and decode from a (large) file.
pub struct LazyDecoder<'a> {
    reader: &'a mut dyn Read,
    bytes: Vec<u8>,
}

impl<'a> LazyDecoder<'a> {
    const CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MiB, chosen at random by fair dice roll

    pub fn from_file(file: &'a mut fs::File) -> Self {
        Self {
            reader: file,
            bytes: Vec::with_capacity(Self::CHUNK_SIZE),
        }
    }

    /// Consumes enough bytes and skip the next CBOR element.
    pub fn skip(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.with_decoder(|d| Ok(d.skip()?))
    }

    /// Consumes enough bytes and decode the next CBOR element.
    pub fn decode<T: for<'d> cbor::decode::Decode<'d, ()>>(
        &mut self,
    ) -> Result<T, Box<dyn std::error::Error>> {
        self.with_decoder(|d| Ok(d.decode()?))
    }

    /// Decode some element according to a custom strategy. This consumes more bytes if the decoder
    /// fails due to a lack of bytes. And error otherwise.
    pub fn with_decoder<T>(
        &mut self,
        decode: impl Fn(&mut cbor::decode::Decoder<'_>) -> Result<T, Box<dyn std::error::Error>>,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let mut should_read_more = self.bytes.is_empty();
        let mut can_read_more = true;
        loop {
            if should_read_more {
                let mut buf = [0; Self::CHUNK_SIZE];
                let read = self
                    .reader
                    .read(&mut buf)
                    .map_err(cbor::decode::Error::custom)?;
                self.bytes.extend_from_slice(&buf);
                can_read_more = read > 0;
            }

            let mut d = cbor::Decoder::new(&self.bytes);

            match decode(&mut d) {
                Ok(value) => {
                    #[cfg(feature = "tracing")]
                    if self.bytes.len() > 100 * Self::CHUNK_SIZE {
                        tracing::warn!(
                            target = std::any::type_name::<T>(),
                            chunk_size = self.bytes.len(),
                            hint = "consider decoding incrementally and/or in smaller chunks",
                            "decoding large chunk"
                        );
                    }
                    self.bytes = Vec::from(&self.bytes[d.position()..]);
                    return Ok(value);
                }
                Err(err) if can_read_more => match err.downcast::<cbor::decode::Error>() {
                    Ok(err) if err.is_end_of_input() => {
                        should_read_more = true;
                        continue;
                    }
                    Ok(err) => return Err(err),
                    Err(err) => return Err(err),
                },
                Err(err) => return Err(err),
            }
        }
    }
}
