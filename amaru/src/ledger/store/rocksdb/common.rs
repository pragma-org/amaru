use pallas_codec::minicbor::{self as cbor};

/// Length of the prefix, here as a constant to keep is consistent across other constants and
/// database options.
pub const PREFIX_LEN: usize = 4;

/// Serialize some value to be used as key, with the given prefix. We use common prefixes for
/// objects of the same type to emulate some kind of table organization within RocksDB. This
/// allows to iterate over the store by specific prefixes and also avoid key-clashes across
/// objects that could otherwise have an identical key.
pub fn as_key<T: cbor::Encode<()> + std::fmt::Debug>(prefix: &[u8], key: T) -> Vec<u8> {
    as_bytes(prefix, key)
}

/// A simple helper function to encode any (serialisable) value to CBOR bytes.
pub fn as_value<T: cbor::Encode<()> + std::fmt::Debug>(value: T) -> Vec<u8> {
    as_bytes(&[], value)
}

/// A simple helper function to encode any (serialisable) value to CBOR bytes.
pub fn as_bytes<T: cbor::Encode<()> + std::fmt::Debug>(prefix: &[u8], value: T) -> Vec<u8> {
    let mut buffer = Vec::from(prefix);
    cbor::encode(value, &mut buffer)
        .unwrap_or_else(|e| panic!("unable to encode value to CBOR: {e:?}"));
    buffer
}
