use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, PartialEq)]
pub struct Bytes {
    pub bytes: Vec<u8>,
}

impl Serialize for Bytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize_bytes(&self.bytes, serializer)
    }
}

impl<'de> Deserialize<'de> for Bytes {
    fn deserialize<D>(deserializer: D) -> Result<Bytes, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize_bytes(deserializer).map(|bytes| Bytes { bytes })
    }
}

impl From<Vec<u8>> for Bytes {
    fn from(bytes: Vec<u8>) -> Self {
        Bytes { bytes }
    }
}

impl From<Bytes> for Vec<u8> {
    fn from(bytes: Bytes) -> Self {
        bytes.bytes
    }
}

fn serialize_bytes<S>(bytes: &Vec<u8>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let buf = hex::encode(bytes);
    serializer.serialize_str(&buf)
}

fn deserialize_bytes<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
where
    D: Deserializer<'de>,
{
    let buf = <String>::deserialize(deserializer)?;
    let bytes = hex::decode(buf).map_err(serde::de::Error::custom)?;
    Ok(bytes)
}
