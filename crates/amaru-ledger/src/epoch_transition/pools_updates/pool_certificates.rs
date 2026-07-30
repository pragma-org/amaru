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

use amaru_kernel::{Epoch, PoolParams, cbor};

/// List of registration / retirement updates collected at the end of an epoch.
/// It can be summarized as the effect to perform at the current epoch + the remaining updates to
/// apply in future epochs (these can only be future retirements). See [`PendingPoolCertificates`]
/// for the result of this summarization.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct PoolCertificates(Vec<PoolCertificate>);

impl<C> cbor::encode::Encode<C> for PoolCertificates {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // NOTE: We explicitly enforce the use of *indefinite* arrays here because it allows us
        // to extend the serialized data easily without having to deserialise it (`Row::extend`
        // appends a certificate by splicing it in front of the trailing break byte).
        e.begin_array()?;
        for certificate in self.0.iter() {
            e.encode_with(certificate, ctx)?;
        }
        e.end()?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolCertificates {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let mut certificates = Vec::new();
        for item in &mut d.array_iter()? {
            certificates.push(item?);
        }
        Ok(Self(certificates))
    }
}

impl PoolCertificates {
    pub fn append_certificate(&mut self, certificate: PoolCertificate) {
        self.0.push(certificate);
    }

    pub fn append_registration(&mut self, params: PoolParams) {
        self.0.push(PoolCertificate::Registration(params));
    }

    pub fn append_retirement(&mut self, epoch: Epoch) {
        self.0.push(PoolCertificate::Retirement(epoch))
    }

    pub fn with_registration(mut self, params: PoolParams) -> Self {
        self.0.push(PoolCertificate::Registration(params));
        self
    }

    pub fn with_retirement(mut self, epoch: Epoch) -> Self {
        self.0.push(PoolCertificate::Retirement(epoch));
        self
    }

    /// Collapse stake pool certificates according to the current epoch. The stable DB is at most k
    /// blocks in the past. So, if a certificate is submitted near the end (i.e. within k blocks) of the
    /// last epoch, then we could be in a situation where we haven't yet processed the registrations
    /// (since they're processed with a delay of k blocks) but have already moved into the next epoch.
    ///
    /// The certificates are folded in submission order, applying two cancellation rules:
    ///
    /// a. Any re-registration that comes after a retirement cancels that retirement.
    /// b. Any retirement that comes after a retirement cancels that previous retirement.
    ///
    pub fn pending_after(&self, current_epoch: Epoch) -> PendingPoolCertificates<'_> {
        use PoolCertificate::*;

        let mut folded = PendingPoolCertificates {
            certificate: None,
            next_certificates: Vec::new(),
            has_resolved_certificates: false,
        };

        for certificate in self.0.iter() {
            match certificate {
                // A re-registration that should now be applied. It overwrites any prior update and
                // cancels any pending retirement, including those scheduled for later epochs.
                Registration(_) => {
                    folded.next_certificates = Vec::new();
                    folded.certificate = Some(certificate);
                }
                Retirement(effective_in) => {
                    if effective_in <= &current_epoch {
                        // A retirement taking effect *now*. It cancels any pool update and supersedes any
                        // earlier retirement.
                        folded.next_certificates = Vec::new();
                        folded.certificate = Some(certificate);
                    } else {
                        // Not effective yet. This certificate is kept for a later epoch. Being submitted afterwards,
                        // it also supersedes any retirement that would otherwise take effect now.
                        if matches!(folded.certificate, Some(Retirement(_))) {
                            folded.certificate = None;
                        }
                        folded.next_certificates.push(certificate);
                    }
                }
            }
        }

        folded.has_resolved_certificates = folded.next_certificates.len() < self.0.len();

        folded
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PoolCertificate {
    Registration(PoolParams),
    Retirement(Epoch),
}

impl<C> cbor::encode::Encode<C> for PoolCertificate {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        // This used to be encoded as `(Option<PoolParams>, Epoch)` tuple, where a
        // retirement is an absent parameters update. We now encode in a similar manner to allow
        // decoding from a 'legacy' format more easily.
        match self {
            Self::Registration(params) => {
                e.array(1)?;
                e.encode_with(params, ctx)?;
            }
            Self::Retirement(at) => {
                e.array(2)?;
                e.null()?;
                e.encode_with(at, ctx)?;
            }
        }
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolCertificate {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d
            .array()?
            .ok_or_else(|| cbor::decode::Error::message("expected definite length when decoding PoolCertificate"))?;

        if d.datatype()? == cbor::data::Type::Null {
            d.null()?;
            Ok(Self::Retirement(d.decode_with(ctx)?))
        } else {
            let params = d.decode_with(ctx)?;
            if len == 2 {
                d.skip()?; // Legacy epoch that may be present.
            }
            Ok(Self::Registration(params))
        }
    }
}

/// The outcome of collapsing a pool's future parameters at a given epoch.
#[derive(Debug)]
pub struct PendingPoolCertificates<'a> {
    /// New certificate becoming active at the current epoch: either a registration or a retirement.
    certificate: Option<&'a PoolCertificate>,

    /// The certificates that remain relevant beyond the current epoch: pending retirements and any
    /// yet-to-be-applied updates, minus those cancelled by a subsequent certificate.
    next_certificates: Vec<&'a PoolCertificate>,

    has_resolved_certificates: bool,
}

impl<'a> PendingPoolCertificates<'a> {
    pub fn registration(&self) -> Option<&'a PoolParams> {
        if let Some(PoolCertificate::Registration(params)) = self.certificate { Some(params) } else { None }
    }

    pub fn is_retiring(&self) -> bool {
        matches!(self.certificate, Some(PoolCertificate::Retirement(_)))
    }

    pub fn is_retiring_at(&self, epoch: Epoch) -> bool {
        matches!(self.certificate, Some(PoolCertificate::Retirement(e)) if *e == epoch)
    }

    pub fn to_next_certificates(&self) -> PoolCertificates {
        // Only retirements can be applied at a future epoch and they are cheap to clone.
        PoolCertificates(self.next_certificates.iter().copied().cloned().collect())
    }

    pub fn has_resolved_certificates(&self) -> bool {
        self.has_resolved_certificates
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use PoolCertificate::*;
    use amaru_kernel::{any_pool_params, prop_cbor_roundtrip};
    use proptest::{collection, prelude::*};

    use super::*;

    pub fn any_pool_certificate(epoch: Epoch) -> impl Strategy<Value = PoolCertificate> {
        prop_oneof![Just(Retirement(epoch)), any_pool_params().prop_map(Registration)]
    }

    pub fn any_pool_certificates() -> impl Strategy<Value = PoolCertificates> {
        collection::vec(0..3u64, 0..3)
            .prop_flat_map(|epochs| {
                epochs.into_iter().map(|e| any_pool_certificate(Epoch::from(e))).collect::<Vec<_>>()
            })
            .prop_map(PoolCertificates)
    }

    prop_cbor_roundtrip!(prop_cbor_roundtrip_pool_certificate, PoolCertificate, any_pool_certificate(Epoch::from(42)));

    prop_cbor_roundtrip!(prop_cbor_roundtrip_pool_certificates, PoolCertificates, any_pool_certificates());

    proptest! {
        // The on-disk format predates this type: certificates must keep encoding exactly like the
        // `(Option<PoolParams>, Epoch)` tuples found in already-stored pool rows.
        #[test]
        fn prop_decodes_legacy_tuple_encoding(certificate in any_pool_certificate(Epoch::from(42))) {
            let legacy: (Option<&PoolParams>, Epoch) = match &certificate {
                Registration(params) => (Some(params), Epoch::new(999)),
                Retirement(at) => (None, *at),
            };

            prop_assert_eq!(amaru_kernel::from_cbor(&amaru_kernel::to_cbor(&legacy)), Some(certificate))
        }
    }
}
