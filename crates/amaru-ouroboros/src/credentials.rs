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

//! Operational certificates and startup checks for block producers.

use std::{fs, path::Path};

use amaru_kernel::{
    Bytes, Ed25519Signature, Hash, Header, HeaderBody, HeaderHash, KesPeriod, KesSignature, OperationalCert, Point,
    ProtocolVersion, VerificationKey, VrfCert, cardano::fixed_bytes::FixedBytes, cbor, ed25519,
    protocol_version::PROTOCOL_VERSION_12, size::BLOCK_BODY,
};
use amaru_ouroboros_traits::{ChainStore, ForgingCredentials, StoreError};
use serde::Deserialize;
use thiserror::Error;

use crate::{praos::header::AssertOperationalCertificateError, vrf::SecretKeyError as VrfKeyError};

const OPERATIONAL_CERTIFICATE_ENVELOPE: &str = "NodeOperationalCertificate";

/// Fail startup when the adopted chain would reject this operational certificate.
///
/// The cold signature must verify. The sequence number must be the counter stored
/// for this pool as of `tip`, or exactly one ahead. A pool with no counter yet may
/// use sequence 0 or 1. The header at `tip` counts: the counter is read from a
/// probe whose parent is that tip, so the tip's own certificate is included.
pub fn ensure_operational_certificate_accepted(
    credentials: &dyn ForgingCredentials,
    store: &dyn ChainStore,
    tip: &Point,
) -> Result<(), CertificateRejected> {
    let fields = credentials.issuer_fields();
    let issuer = cold_verifying_key(&fields.issuer_verification_key).ok_or(CertificateRejected::ColdKey)?;
    let latest = match tip {
        Point::Origin => None,
        Point::Specific(_, hash, _) => {
            let probe = header_parented_on(*hash);
            store.get_latest_opcert_sequence_number(&credentials.pool_id(), &probe)?
        }
    };
    AssertOperationalCertificateError::new(&fields.operational_cert, &issuer, latest)
        .map_err(|error| CertificateRejected::Rejected(Box::new(error)))
}

fn cold_verifying_key(cold: &VerificationKey) -> Option<ed25519::VerifyingKey> {
    ed25519::VerifyingKey::try_from(cold.as_slice()).ok()
}

/// Header whose parent is `parent`. The chain store reads the opcert counter as of that parent.
fn header_parented_on(parent: HeaderHash) -> Header {
    Header::new(
        HeaderBody {
            block_number: 0,
            slot: 0,
            prev_hash: Some(parent),
            issuer_verification_key: VerificationKey::zeroes(),
            vrf_verification_key: VerificationKey::zeroes(),
            vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
            block_body_size: 0,
            block_body_hash: Hash::new([0u8; BLOCK_BODY]),
            operational_cert: OperationalCert {
                operational_cert_hot_verification_key: VerificationKey::zeroes(),
                operational_cert_sequence_number: 0,
                operational_cert_kes_period: KesPeriod::from(0),
                operational_cert_sigma: Ed25519Signature::zeroes(),
            },
            protocol_version: ProtocolVersion::new(11, 0),
        },
        KesSignature::zeroes(),
    )
}

/// Verify the cold key and signature without consulting the chain's certificate counter.
pub(crate) fn validate_operational_certificate(
    certificate: &OperationalCert,
    cold: &VerificationKey,
) -> Result<(), CredentialsError> {
    let issuer = cold_verifying_key(cold).ok_or(CredentialsError::ColdKey)?;
    AssertOperationalCertificateError::new(certificate, &issuer, Some(certificate.operational_cert_sequence_number))
        .map_err(|error| CredentialsError::Signature(error.to_string()))
}

pub(crate) fn load_operational_certificate(
    path: impl AsRef<Path>,
) -> Result<(OperationalCert, VerificationKey), OperationalCertificateError> {
    let bytes = fs::read(path)?;
    let envelope: OperationalCertificateEnvelope = serde_json::from_slice(&bytes)?;
    if envelope.r#type != OPERATIONAL_CERTIFICATE_ENVELOPE {
        return Err(OperationalCertificateError::UnexpectedEnvelopeType {
            expected: OPERATIONAL_CERTIFICATE_ENVELOPE,
            found: envelope.r#type,
        });
    }
    let payload = hex::decode(envelope.cbor_hex)?;
    decode_operational_certificate(&payload)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OperationalCertificateEnvelope {
    r#type: String,
    cbor_hex: String,
}

fn decode_operational_certificate(
    payload: &[u8],
) -> Result<(OperationalCert, VerificationKey), OperationalCertificateError> {
    let mut decoder = cbor::Decoder::new(payload);
    let mut ctx = PROTOCOL_VERSION_12;
    match decoder.array().map_err(OperationalCertificateError::cbor)? {
        Some(2) => {}
        Some(len) => return Err(OperationalCertificateError::ArrayLength(len)),
        None => return Err(OperationalCertificateError::IndefiniteArray),
    }
    let certificate = decoder.decode_with(&mut ctx).map_err(OperationalCertificateError::cbor)?;
    let cold = decoder.decode_with(&mut ctx).map_err(OperationalCertificateError::cbor)?;
    if decoder.position() != payload.len() {
        return Err(OperationalCertificateError::TrailingBytes(payload.len() - decoder.position()));
    }
    Ok((certificate, cold))
}

/// Why the forging files were refused before the node serves traffic.
#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("could not start KES signer: {0}")]
    Io(#[from] std::io::Error),
    #[error("KES signer: {0}")]
    Signer(String),
    #[error("VRF signing key: {0}")]
    Vrf(#[from] VrfKeyError),
    #[error("operational certificate: {0}")]
    Certificate(#[from] OperationalCertificateError),
    #[error("operational certificate cold verification key is not a valid Ed25519 key")]
    ColdKey,
    #[error("operational certificate signature: {0}")]
    Signature(String),
    #[error(
        "block forging key files are refused on mainnet; they are available on preprod, preview, and other testnets"
    )]
    Mainnet,
    #[error("block forging needs --operator-kes, --operator-vrf, and --operator-cert together")]
    PartialFlags,
}

/// The operational certificate would not be accepted on the adopted chain.
#[derive(Debug, Error)]
pub enum CertificateRejected {
    #[error("could not read the operational certificate counter from the chain store: {0}")]
    Store(#[from] StoreError),
    #[error("operational certificate is not valid for the adopted chain: {0}")]
    Rejected(Box<AssertOperationalCertificateError>),
    #[error("operational certificate cold verification key is not a valid Ed25519 key")]
    ColdKey,
}

/// The operational-certificate text envelope could not be read.
#[derive(Debug, Error)]
pub enum OperationalCertificateError {
    #[error("unexpected operational certificate envelope type: expected {expected}, found {found}")]
    UnexpectedEnvelopeType { expected: &'static str, found: String },
    #[error("operational certificate CBOR must be a definite two-element array")]
    IndefiniteArray,
    #[error("operational certificate CBOR must be a two-element array, found {0} elements")]
    ArrayLength(u64),
    #[error("operational certificate CBOR has {0} trailing bytes")]
    TrailingBytes(usize),
    #[error("operational certificate CBOR is malformed: {0}")]
    Cbor(String),
    #[error("failed to read operational certificate file: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed operational certificate envelope: {0}")]
    Envelope(#[from] serde_json::Error),
    #[error("operational certificate hex is malformed: {0}")]
    Hex(#[from] hex::FromHexError),
}

impl OperationalCertificateError {
    fn cbor(error: impl ToString) -> Self {
        Self::Cbor(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use amaru_kernel::{IsHeader, cbor, ed25519::Signer};
    use amaru_ouroboros_traits::{ForgingCredentialsError, InMemoryChainStore, IssuerFields, WriteChainStore};
    use serde::Deserialize;

    use super::*;
    use crate::{kes, vrf};

    fn envelope(r#type: &str, cbor_hex: &str) -> String {
        format!(r#"{{"type":"{type}","description":"","cborHex":"{cbor_hex}"}}"#)
    }

    struct Issued {
        certificate: OperationalCert,
        cold_vk: VerificationKey,
    }

    fn issue(sequence: u64, start: KesPeriod) -> Issued {
        let cold = ed25519::SigningKey::from_bytes(&[9u8; 32]);
        let mut kes = kes::SecretKey::for_tests();
        let hot = VerificationKey::from(*kes::PublicKey::from(&mut kes));
        let mut message = Vec::with_capacity(48);
        message.extend_from_slice(&hot[..]);
        message.extend_from_slice(&sequence.to_be_bytes());
        message.extend_from_slice(&u64::from(start).to_be_bytes());
        let certificate = OperationalCert {
            operational_cert_hot_verification_key: hot,
            operational_cert_sequence_number: sequence,
            operational_cert_kes_period: start,
            operational_cert_sigma: Ed25519Signature::from(cold.sign(&message).to_bytes()),
        };
        let cold_vk = VerificationKey::from(cold.verifying_key().to_bytes());
        Issued { certificate, cold_vk }
    }

    fn corrupt_signature(certificate: &mut OperationalCert) {
        let mut sigma = *certificate.operational_cert_sigma.as_array();
        sigma[0] ^= 0xff;
        certificate.operational_cert_sigma = Ed25519Signature::from(sigma);
    }

    fn certificate_cbor(certificate: &OperationalCert, cold: &VerificationKey) -> String {
        let mut buf = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut buf);
        let mut ctx = PROTOCOL_VERSION_12;
        encoder.array(2).unwrap();
        encoder.encode_with(certificate, &mut ctx).unwrap();
        encoder.encode_with(cold, &mut ctx).unwrap();
        hex::encode(buf)
    }

    fn forging_fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/forging").join(name)
    }

    fn cbor_byte_string(path: &Path) -> Vec<u8> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Envelope {
            cbor_hex: String,
        }
        let envelope: Envelope = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        let payload = hex::decode(envelope.cbor_hex).unwrap();
        let mut decoder = cbor::Decoder::new(&payload);
        cbor::decode_bytes(&mut decoder).unwrap().into_owned()
    }

    /// Throwaway keys from `cardano-cli conway node` (`key-gen-VRF`, `key-gen-KES`, `key-gen`,
    /// `issue-op-cert`). Not a live stake pool.
    #[test]
    fn cardano_cli_conway_fixtures_load_and_verify() {
        let kes_path = forging_fixture("kes.skey");
        let vrf_path = forging_fixture("vrf.skey");
        let cert_path = forging_fixture("node.cert");
        let kes_vk = cbor_byte_string(&forging_fixture("kes.vkey"));
        let vrf_vk = cbor_byte_string(&forging_fixture("vrf.vkey"));
        let cold_vk = cbor_byte_string(&forging_fixture("cold.vkey"));

        let mut kes = kes::SecretKey::from_file(&kes_path).unwrap();
        assert_eq!(u32::from(kes.period()), 0, "a fresh cardano-cli KES file has no period");
        assert_eq!(kes::PublicKey::from(&mut kes).as_ref(), kes_vk.as_slice());

        let vrf = vrf::SecretKey::from_file(&vrf_path).unwrap();
        assert_eq!(vrf::PublicKey::from(&vrf).as_ref(), vrf_vk.as_slice());
        let input = vrf::Input::from(&[11u8; vrf::Input::SIZE]);
        let proof = vrf.prove(&input);
        proof.verify(&vrf::PublicKey::from(&vrf), &input).unwrap();

        let (certificate, cold) = load_operational_certificate(&cert_path).unwrap();
        validate_operational_certificate(&certificate, &cold).unwrap();
        assert_eq!(cold.as_slice(), cold_vk.as_slice());
        assert_eq!(certificate.operational_cert_hot_verification_key.as_slice(), kes_vk.as_slice());
        assert_eq!(certificate.operational_cert_sequence_number, 0);
        assert_eq!(u64::from(certificate.operational_cert_kes_period), 7);
    }

    #[test]
    fn bad_cold_signature_is_rejected() {
        let mut issued = issue(1, KesPeriod::from(0));
        corrupt_signature(&mut issued.certificate);
        let err = validate_operational_certificate(&issued.certificate, &issued.cold_vk).unwrap_err();
        assert!(matches!(err, CredentialsError::Signature(_)), "{err}");
    }

    #[test]
    fn wrong_envelope_type_and_truncated_cbor_fail() {
        let issued = issue(0, KesPeriod::from(0));
        let wrong =
            envelope("NodeOperationalCertificateIssue", &certificate_cbor(&issued.certificate, &issued.cold_vk));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("node.cert");
        fs::write(&path, wrong).unwrap();
        let err = load_operational_certificate(&path).unwrap_err();
        assert!(matches!(err, OperationalCertificateError::UnexpectedEnvelopeType { .. }), "{err}");

        let truncated = envelope("NodeOperationalCertificate", "8200");
        fs::write(&path, truncated).unwrap();
        let err = load_operational_certificate(&path).unwrap_err();
        assert!(matches!(err, OperationalCertificateError::Cbor(_)), "{err}");
    }

    fn store_producer_header(store: &InMemoryChainStore, credentials: &dyn ForgingCredentials, slot: u64) -> Point {
        let issuer = credentials.issuer_fields();
        let header = Header::new(
            HeaderBody {
                block_number: 1,
                slot,
                prev_hash: None,
                issuer_verification_key: issuer.issuer_verification_key,
                vrf_verification_key: issuer.vrf_verification_key,
                vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::zeroes() },
                block_body_size: 0,
                block_body_hash: Hash::new([0u8; BLOCK_BODY]),
                operational_cert: issuer.operational_cert,
                protocol_version: ProtocolVersion::new(11, 0),
            },
            KesSignature::zeroes(),
        );
        store.store_header(&header).unwrap();
        Point::Specific(header.slot(), header.hash(), header.block_height())
    }

    #[test]
    fn startup_accepts_the_stored_counter_or_one_ahead() {
        let on_chain = issue(4, KesPeriod::from(0));
        let store = InMemoryChainStore::new();
        let tip = store_producer_header(&store, &on_chain, 100);
        ensure_operational_certificate_accepted(&on_chain, &store, &tip).unwrap();

        let next = issue(5, KesPeriod::from(0));
        ensure_operational_certificate_accepted(&next, &store, &tip).unwrap();

        let ahead = issue(6, KesPeriod::from(0));
        let err = ensure_operational_certificate_accepted(&ahead, &store, &tip).unwrap_err();
        assert!(err.to_string().contains("too far ahead"), "{err}");

        let behind = issue(3, KesPeriod::from(0));
        let err = ensure_operational_certificate_accepted(&behind, &store, &tip).unwrap_err();
        assert!(err.to_string().contains("less than"), "{err}");
    }

    #[test]
    fn startup_on_an_empty_chain_accepts_the_first_certificate_only() {
        let store = InMemoryChainStore::new();
        ensure_operational_certificate_accepted(&issue(0, KesPeriod::from(0)), &store, &Point::Origin).unwrap();
        ensure_operational_certificate_accepted(&issue(1, KesPeriod::from(0)), &store, &Point::Origin).unwrap();
        let err =
            ensure_operational_certificate_accepted(&issue(5, KesPeriod::from(0)), &store, &Point::Origin).unwrap_err();
        assert!(err.to_string().contains("too far ahead"), "{err}");
    }

    #[test]
    fn startup_reports_a_cold_signature_the_chain_would_reject() {
        let mut issued = issue(0, KesPeriod::from(0));
        corrupt_signature(&mut issued.certificate);
        let err =
            ensure_operational_certificate_accepted(&issued, &InMemoryChainStore::new(), &Point::Origin).unwrap_err();
        assert!(err.to_string().contains("Invalid operational certificate signature"), "{err}");
    }

    impl ForgingCredentials for Issued {
        fn issuer_verification_key(&self) -> VerificationKey {
            self.cold_vk
        }

        fn vrf_secret_bytes(&self) -> [u8; 32] {
            [0u8; 32]
        }

        fn issuer_fields(&self) -> IssuerFields {
            IssuerFields {
                issuer_verification_key: self.cold_vk,
                vrf_verification_key: VerificationKey::zeroes(),
                operational_cert: self.certificate.clone(),
            }
        }

        fn sign(&self, _period: KesPeriod, _message: &[u8]) -> Result<KesSignature, ForgingCredentialsError> {
            Ok(KesSignature::zeroes())
        }
    }
}
