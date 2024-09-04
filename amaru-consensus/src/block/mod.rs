use amaru_common::ledger::{PoolIdToSigma, Sigma};
use amaru_common::pool::issuer_vkey_to_pool_id;
use amaru_common::validator::Validator;
use log::warn;
use pallas_traverse::MultiEraHeader;

struct BlockValidator<'b> {
    multi_era_header: MultiEraHeader<'b>,
    pool_id_to_sigma: &'b dyn PoolIdToSigma,
}

impl<'b> BlockValidator<'b> {
    fn new(multi_era_header: MultiEraHeader<'b>, pool_id_to_sigma: &'b dyn PoolIdToSigma) -> Self {
        Self { multi_era_header, pool_id_to_sigma }
    }

    fn validate_epoch_boundary(&self) -> bool {
        // we ignore epoch boundaries and assume they are valid
        true
    }

    fn validate_byron(&self) -> bool {
        // we ignore byron headers and assume they are valid
        true
    }

    fn validate_shelley_compatible(&self) -> bool {
        let Some(issuer_vkey) = self.multi_era_header.issuer_vkey();
        let pool_id = issuer_vkey_to_pool_id(issuer_vkey);
        let sigma = match self.pool_id_to_sigma.sigma(&pool_id) {
            Ok(sigma) => sigma,
            Err(error) => {
                warn!("{:?} - {:?}", error, pool_id);
                return false;
            }
        };
        let slot = self.multi_era_header.slot();
        let Ok(leader_vrf_output) = self.multi_era_header.leader_vrf_output();
        let Ok(vrf_vkey) = self.multi_era_header.vrf_vkey();

        true
    }

    fn validate_babbage_compatible(&self) -> bool {
        let cbor = self.multi_era_header.cbor();
        true
    }
}

impl Validator for BlockValidator<'_> {
    fn validate(&self) -> bool {
        match self.multi_era_header {
            MultiEraHeader::EpochBoundary(_) => self.validate_epoch_boundary(),
            MultiEraHeader::Byron(_) => self.validate_byron(),
            MultiEraHeader::ShelleyCompatible(_) => self.validate_shelley_compatible(),
            MultiEraHeader::BabbageCompatible(_) => self.validate_babbage_compatible(),
        }
    }
}