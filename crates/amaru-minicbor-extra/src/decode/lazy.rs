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

use std::io::Read;

use minicbor as cbor;

/// A decoder that only consumes bytes CHUNK_SIZE at a time. Useful to decode large files while
/// maintaining memory usage low.
///
/// The decoder keeps an internal state with the bytes that have been read but not consumed, and a
/// handle to a source that implements [`std::io::Read`].
///
pub struct LazyDecoder<'a> {
    reader: &'a mut dyn Read,
    bytes: Vec<u8>,
}

impl<'a> LazyDecoder<'a> {
    const CHUNK_SIZE: usize = 2 * 1024 * 1024; // 2MiB, chosen at random by fair dice roll

    /// Create a decoder that reads incrementally from `reader`.
    pub fn new(reader: &'a mut dyn Read) -> Self {
        Self { reader, bytes: Vec::with_capacity(Self::CHUNK_SIZE) }
    }

    /// Skip the next CBOR element.
    ///
    /// Arrays and maps are consumed incrementally so the complete container does not need to fit
    /// in memory. Other values are skipped by [`cbor::Decoder::skip`].
    pub fn skip(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let datatype = self.with_decoder(|d| Ok(d.datatype()?))?;

        if matches!(datatype, cbor::data::Type::Array | cbor::data::Type::ArrayIndef) {
            self.skip_array()
        } else if matches!(datatype, cbor::data::Type::Map | cbor::data::Type::MapIndef) {
            self.skip_map()
        } else {
            self.with_decoder(|d| Ok(d.skip()?))
        }
    }

    fn skip_array(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let length = self.with_decoder(|d| Ok(d.array()?))?;
        self.skip_entries(length, |d| d.skip())
    }

    fn skip_map(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let length = self.with_decoder(|d| Ok(d.map()?))?;
        self.skip_entries(length, |d| {
            d.skip()?;
            d.skip()
        })
    }

    fn skip_entries(
        &mut self,
        mut remaining: Option<u64>,
        skip_entry: impl Fn(&mut cbor::Decoder<'_>) -> Result<(), cbor::decode::Error>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        loop {
            let (entries, complete) =
                self.with_decoder(|d| decode_sequence_chunk(d, remaining, &skip_entry).map_err(Into::into))?;
            remaining = remaining.map(|count| count - entries.len() as u64);

            if complete {
                return Ok(());
            }
        }
    }

    /// Consume enough bytes to decode the next CBOR element.
    pub fn decode<T: for<'d> cbor::decode::Decode<'d, ()>>(&mut self) -> Result<T, Box<dyn std::error::Error>> {
        self.with_decoder(|d| Ok(d.decode()?))
    }

    /// Consume the header of the next definite- or indefinite-length CBOR array.
    pub fn begin_array(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.with_decoder(|d| {
            d.array()?;
            Ok(())
        })
    }

    /// Decode an element with a custom strategy.
    ///
    /// More bytes are read and the strategy is retried when it reaches the end of the available
    /// input. Other errors are returned unchanged.
    pub fn with_decoder<T>(
        &mut self,
        decode: impl Fn(&mut cbor::decode::Decoder<'_>) -> Result<T, Box<dyn std::error::Error>>,
    ) -> Result<T, Box<dyn std::error::Error>> {
        let mut should_read_more = self.bytes.is_empty();
        let mut can_read_more = true;
        loop {
            if should_read_more {
                let offset = self.bytes.len();
                self.bytes.resize(offset + Self::CHUNK_SIZE, 0);
                let read = match self.reader.read(&mut self.bytes[offset..]) {
                    Ok(read) => read,
                    Err(error) => {
                        self.bytes.truncate(offset);
                        return Err(cbor::decode::Error::custom(error).into());
                    }
                };
                self.bytes.truncate(offset + read);
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

    /// Decode a CBOR map in batches without waiting for the entire map to be buffered.
    ///
    /// Both definite- and indefinite-length maps are accepted. `init` receives the map's declared
    /// length, or `None` for an indefinite-length map. `decode_entry` may be retried after more
    /// input is read and must not have external side effects. `handle_entries` receives each chunk
    /// exactly once, so it may safely update the state or perform external side effects. An empty
    /// map produces one empty chunk. If the handler fails, the decoder remains positioned after
    /// the chunk it received.
    pub fn stream_map<T, S>(
        &mut self,
        decode_entry: impl Fn(&mut cbor::Decoder<'_>) -> Result<T, cbor::decode::Error>,
        init: impl FnOnce(Option<u64>) -> S,
        mut handle_entries: impl FnMut(&mut S, Vec<T>) -> Result<(), Box<dyn std::error::Error>>,
    ) -> Result<S, Box<dyn std::error::Error>> {
        let length = self.with_decoder(|d| Ok(d.map()?))?;
        let mut remaining = length;
        let mut state = init(length);

        loop {
            let (entries, complete) =
                self.with_decoder(|d| decode_sequence_chunk(d, remaining, &decode_entry).map_err(Into::into))?;

            remaining = remaining.map(|count| count - entries.len() as u64);
            handle_entries(&mut state, entries)?;

            if complete {
                return Ok(state);
            }
        }
    }
}

/// Decode as many complete array elements or map entries as possible from the current input.
///
/// `remaining` is the number of items left in a definite-length container, or `None` for an
/// indefinite-length container. The returned boolean is `true` when the end of the container was
/// consumed. An incomplete item is left untouched so decoding can resume after more input becomes
/// available. The caller must consume the container header before calling this function.
///
/// The `decode_entry` callback may be retried and must not have external side effects.
fn decode_sequence_chunk<T>(
    decoder: &mut cbor::Decoder<'_>,
    remaining: Option<u64>,
    decode_entry: &impl Fn(&mut cbor::Decoder<'_>) -> Result<T, cbor::decode::Error>,
) -> Result<(Vec<T>, bool), cbor::decode::Error> {
    let mut entries = Vec::new();

    loop {
        let remaining = remaining.map(|count| count - entries.len() as u64);
        if remaining == Some(0) {
            return Ok((entries, true));
        }
        if remaining.is_none() {
            match crate::decode_break(decoder, None) {
                Ok(true) => return Ok((entries, true)),
                Ok(false) => {}
                Err(error) if error.is_end_of_input() && !entries.is_empty() => return Ok((entries, false)),
                Err(error) => return Err(error),
            }
        }

        let decoded = {
            let mut probe = decoder.probe();
            decode_entry(&mut probe).map(|entry| (entry, probe.position()))
        };
        match decoded {
            Ok((entry, position)) => {
                decoder.set_position(position);
                entries.push(entry);
            }
            Err(error) if error.is_end_of_input() && !entries.is_empty() => return Ok((entries, false)),
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Read};

    use minicbor as cbor;

    use super::{LazyDecoder, decode_sequence_chunk};

    struct ChunkedReader<R> {
        inner: R,
        chunk_size: usize,
    }

    impl<R: Read> Read for ChunkedReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let len = buffer.len().min(self.chunk_size);
            self.inner.read(&mut buffer[..len])
        }
    }

    #[test]
    fn decode_sequence_chunk_leaves_an_incomplete_entry_untouched() {
        let mut decoder = cbor::Decoder::new(&[0x01, 0x0a, 0x02]);

        let (entries, complete) =
            decode_sequence_chunk(&mut decoder, Some(2), &|d| Ok((d.u8()?, d.u8()?))).expect("valid first entry");

        assert_eq!(entries, vec![(1, 10)]);
        assert!(!complete);
        assert_eq!(decoder.position(), 2);
    }

    #[test]
    fn decodes_definite_and_indefinite_maps_incrementally() {
        let cases: &[&[u8]] = &[
            &[0xa3, 0x03, 0x18, 0x1e, 0x01, 0x0a, 0x02, 0x14],
            &[0xbf, 0x03, 0x18, 0x1e, 0x01, 0x0a, 0x02, 0x14, 0xff],
        ];

        for bytes in cases {
            let mut reader = ChunkedReader { inner: *bytes, chunk_size: 2 };
            let mut decoder = LazyDecoder::new(&mut reader);

            let (entries, folds) = decoder
                .stream_map(
                    |d| Ok((d.u8()?, d.u8()?)),
                    |_| (Vec::new(), 0),
                    |(entries, folds), chunk| {
                        *folds += chunk.len();
                        entries.extend(chunk);
                        Ok(())
                    },
                )
                .expect("valid map");

            assert_eq!(entries, vec![(3, 30), (1, 10), (2, 20)]);
            assert_eq!(folds, 3);
        }
    }

    #[test]
    fn reports_the_declared_length_for_an_empty_map() {
        let bytes = [0xa0];
        let mut reader = bytes.as_slice();
        let mut decoder = LazyDecoder::new(&mut reader);
        let (length, chunks) = decoder
            .stream_map(
                |d| Ok((d.u8()?, d.u8()?)),
                |length| (length, Vec::new()),
                |(length, chunks), entries| {
                    chunks.push((*length, entries));
                    Ok(())
                },
            )
            .expect("valid empty map");

        assert_eq!(length, Some(0));
        assert_eq!(chunks, vec![(Some(0), Vec::<(u8, u8)>::new())]);
    }

    #[test]
    fn skips_a_map_without_consuming_the_next_item() {
        let bytes = [0x82, 0xbf, 0x01, 0x82, 0x02, 0x03, 0xff, 0x18, 0x2a];
        let mut reader = ChunkedReader { inner: bytes.as_slice(), chunk_size: 2 };
        let mut decoder = LazyDecoder::new(&mut reader);

        decoder.begin_array().expect("valid array");
        decoder.skip().expect("valid map");

        assert_eq!(decoder.decode::<u8>().expect("trailing integer"), 42);
    }

    #[test]
    fn skips_nested_definite_and_indefinite_containers() {
        let cases: &[&[u8]] = &[
            &[0x82, 0x81, 0x01, 0xa1, 0x02, 0x03, 0x18, 0x2a],
            &[0x9f, 0xbf, 0x01, 0x9f, 0x02, 0x03, 0xff, 0xff, 0xff, 0x18, 0x2a],
        ];

        for bytes in cases {
            let mut reader = ChunkedReader { inner: *bytes, chunk_size: 2 };
            let mut decoder = LazyDecoder::new(&mut reader);

            decoder.skip().expect("valid nested container");

            assert_eq!(decoder.decode::<u8>().expect("trailing integer"), 42);
        }
    }

    #[test]
    fn skips_a_tagged_container() {
        let bytes = [0xd9, 0x01, 0x02, 0x9f, 0xa1, 0x01, 0x02, 0xff, 0x18, 0x2a];
        let mut reader = ChunkedReader { inner: bytes.as_slice(), chunk_size: 2 };
        let mut decoder = LazyDecoder::new(&mut reader);

        decoder.skip().expect("valid tagged container");

        assert_eq!(decoder.decode::<u8>().expect("trailing integer"), 42);
    }

    #[test]
    fn rejects_an_indefinite_map_without_a_value() {
        let bytes = [0xbf, 0x01, 0xff];
        let mut reader = ChunkedReader { inner: bytes.as_slice(), chunk_size: 1 };
        let mut decoder = LazyDecoder::new(&mut reader);

        decoder.skip().expect_err("map value is missing");
    }
}
