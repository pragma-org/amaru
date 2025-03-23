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

use crate::{
    context::{
        AccountState, AccountsSlice, CCMember, CommitteeSlice, DRepState, DRepsSlice,
        DelegateError, PoolsSlice, PotsSlice, RegisterError, UnregisterError, UpdateError,
        UtxoSlice, ValidationContext, WitnessSlice,
    },
    state::volatile_db::VolatileState,
};
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, Epoch, Hash, Lovelace, PoolId, PoolParams, StakeCredential,
    TransactionInput, TransactionOutput,
};
use std::collections::{BTreeMap, BTreeSet};
use tracing::trace;

#[derive(Debug)]
pub struct DefaultValidationContext<'a> {
    utxo: BTreeMap<&'a TransactionInput, TransactionOutput>,
    state: VolatileState,
    required_signers: BTreeSet<Hash<28>>,
    required_bootstrap_signers: BTreeSet<Hash<28>>,
}

impl<'a> DefaultValidationContext<'a> {
    pub fn new(utxo: BTreeMap<&'a TransactionInput, TransactionOutput>) -> Self {
        Self {
            utxo,
            state: VolatileState::default(),
            required_signers: BTreeSet::default(),
            required_bootstrap_signers: BTreeSet::default(),
        }
    }
}

impl ValidationContext for DefaultValidationContext<'_> {}

impl PotsSlice for DefaultValidationContext<'_> {
    fn add_fees(&mut self) {
        unimplemented!()
    }
}

impl UtxoSlice for DefaultValidationContext<'_> {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.utxo.get(input).or(self.state.utxo.produced.get(input))
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
        self.state.utxo.consume(input)
    }

    fn produce(&mut self, input: TransactionInput, output: TransactionOutput) {
        self.state.utxo.produce(input, output)
    }
}

impl PoolsSlice for DefaultValidationContext<'_> {
    fn lookup(&self, _pool: &PoolId) -> Option<&PoolParams> {
        unimplemented!()
    }

    fn register(&mut self, params: PoolParams) {
        trace!(?params, "certificate.pool.registration");
        self.state.pools.register(params.id, params)
    }

    fn retire(&mut self, pool: PoolId, epoch: Epoch) {
        trace!(%pool, %epoch, "certificate.pool.retirement");
        self.state.pools.unregister(pool, epoch)
    }
}

impl AccountsSlice for DefaultValidationContext<'_> {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&AccountState> {
        unimplemented!()
    }

    fn register(
        &mut self,
        credential: StakeCredential,
        state: AccountState,
    ) -> Result<(), RegisterError<AccountState, StakeCredential>> {
        trace!(?credential, "certificate.stake.registration"); // TODO: Use Display for Credential
        self.state
            .accounts
            .register(credential, state.deposit, state.pool, state.drep)?;
        Ok(())
    }

    fn delegate_pool(
        &mut self,
        credential: StakeCredential,
        pool: PoolId,
    ) -> Result<(), DelegateError<StakeCredential, PoolId>> {
        trace!(?credential, %pool, "certificate.stake.delegation"); // TODO: Use Display for Credential
        self.state.accounts.bind_left(credential, Some(pool))?;
        Ok(())
    }

    fn delegate_vote(
        &mut self,
        credential: StakeCredential,
        drep: DRep,
        pointer: CertificatePointer,
    ) -> Result<(), DelegateError<StakeCredential, DRep>> {
        trace!(?credential, ?drep, "certificate.vote.delegation");
        self.state
            .accounts
            .bind_right(credential, Some((drep, pointer)))?;
        Ok(())
    }

    fn unregister(&mut self, credential: StakeCredential) {
        trace!(?credential, "certificate.stake.deregistration");
        self.state.accounts.unregister(credential)
    }

    fn withdraw_from(&mut self, _credential: &StakeCredential) {
        unimplemented!()
    }
}

impl DRepsSlice for DefaultValidationContext<'_> {
    fn lookup(&self, _credential: &StakeCredential) -> Option<&DRepState> {
        unimplemented!()
    }

    fn register(
        &mut self,
        drep: StakeCredential,
        state: DRepState,
    ) -> Result<(), RegisterError<DRepState, StakeCredential>> {
        trace!(?drep, deposit = ?state.deposit, anchor = ?state.anchor, "certificate.drep.registration");
        self.state.dreps.register(
            drep,
            (state.deposit, state.registered_at),
            state.anchor,
            None,
        )?;
        Ok(())
    }

    fn update(
        &mut self,
        drep: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UpdateError<StakeCredential>> {
        trace!(?drep, ?anchor, "certificate.drep.update");
        self.state.dreps.bind_left(drep, anchor)?;
        Ok(())
    }

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace) {
        trace!(?drep, ?refund, "certificate.drep.retirement");
        self.state.dreps.unregister(drep)
    }

    fn vote(&mut self, _drep: StakeCredential) {
        unimplemented!()
    }
}

impl CommitteeSlice for DefaultValidationContext<'_> {
    fn delegate_cold_key(
        &mut self,
        cc_member: StakeCredential,
        delegate: StakeCredential,
    ) -> Result<(), DelegateError<StakeCredential, StakeCredential>> {
        trace!(name: "certificate.committee.delegate", ?cc_member, ?delegate);
        self.state.committee.bind_left(cc_member, Some(delegate))?;
        Ok(())
    }

    fn resign(
        &mut self,
        cc_member: StakeCredential,
        anchor: Option<Anchor>,
    ) -> Result<(), UnregisterError<CCMember, StakeCredential>> {
        trace!(name: "certificate.committee.resign", ?cc_member, ?anchor);
        self.state.committee.unregister(cc_member);
        Ok(())
    }
}

impl WitnessSlice for DefaultValidationContext<'_> {
    fn require_witness(&mut self, credential: StakeCredential) {
        match credential {
            StakeCredential::AddrKeyhash(vk_hash) => {
                self.required_signers.insert(vk_hash);
            }
            StakeCredential::ScriptHash(..) => {
                // FIXME: Also account for native scripts. We should pre-fetch necessary scripts
                // before hand, and here, check whether additional signatures are needed.
            }
        }
    }

    fn require_bootstrap_witness(&mut self, root: Hash<28>) {
        self.required_bootstrap_signers.insert(root);
    }

    fn required_signers(&self) -> BTreeSet<Hash<28>> {
        self.required_signers.iter().copied().collect()
    }

    fn required_bootstrap_signers(&self) -> BTreeSet<Hash<28>> {
        self.required_bootstrap_signers.iter().copied().collect()
    }
}
