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
        DelegateError, PoolsSlice, PotsSlice, ProposalsSlice, RegisterError, UnregisterError,
        UpdateError, UtxoSlice, ValidationContext, WitnessSlice,
    },
    state::volatile_db::VolatileState,
};
use amaru_kernel::{
    Anchor, CertificatePointer, DRep, Epoch, Hash, Lovelace, PoolId, PoolParams, Proposal,
    ProposalId, ProposalPointer, StakeCredential, TransactionInput, TransactionOutput,
};
use core::mem;
use std::collections::{BTreeMap, BTreeSet};
use tracing::trace;

#[derive(Debug)]
pub struct DefaultValidationContext<'b> {
    utxo: BTreeMap<TransactionInput, TransactionOutput<'b>>,
    state: VolatileState<'b>,
    required_signers: BTreeSet<Hash<28>>,
    required_bootstrap_signers: BTreeSet<Hash<28>>,
}

impl<'b> DefaultValidationContext<'b> {
    pub fn new(utxo: BTreeMap<TransactionInput, TransactionOutput<'b>>) -> Self {
        Self {
            utxo,
            state: VolatileState::default(),
            required_signers: BTreeSet::default(),
            required_bootstrap_signers: BTreeSet::default(),
        }
    }
}

impl<'a, 'b> From<DefaultValidationContext<'b>> for VolatileState<'a>
where
    'b: 'a,
{
    fn from(ctx: DefaultValidationContext<'b>) -> VolatileState<'a> {
        ctx.state
    }
}

impl<'b> ValidationContext<'b> for DefaultValidationContext<'b> {
    type FinalState = VolatileState<'b>;
}

impl<'b> PotsSlice for DefaultValidationContext<'b> {
    fn add_fees(&mut self, fees: Lovelace) {
        self.state.fees += fees;
    }
}

impl<'b> UtxoSlice<'b> for DefaultValidationContext<'b> {
    fn lookup(&self, input: &TransactionInput) -> Option<&TransactionOutput<'b>> {
        self.utxo.get(input).or(self.state.utxo.produced.get(input))
    }

    fn consume(&mut self, input: TransactionInput) {
        self.utxo.remove(&input);
        self.state.utxo.consume(input)
    }

    fn produce(&mut self, input: TransactionInput, output: TransactionOutput<'b>) {
        self.state.utxo.produce(input, output)
    }
}

impl<'b> PoolsSlice for DefaultValidationContext<'b> {
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

impl<'b> AccountsSlice for DefaultValidationContext<'b> {
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

    fn withdraw_from(&mut self, credential: StakeCredential) {
        self.state.withdrawals.insert(credential);
    }
}

impl<'b> DRepsSlice for DefaultValidationContext<'b> {
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

    fn unregister(&mut self, drep: StakeCredential, refund: Lovelace, pointer: CertificatePointer) {
        trace!(?drep, ?refund, "certificate.drep.retirement");
        self.state
            .dreps_deregistrations
            .insert(drep.clone(), pointer);
        self.state.dreps.unregister(drep)
    }

    fn vote(&mut self, drep: StakeCredential) {
        trace!(?drep, "drep.vote");
        self.state.voting_dreps.insert(drep);
    }
}

impl<'b> CommitteeSlice for DefaultValidationContext<'b> {
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

impl<'b> ProposalsSlice for DefaultValidationContext<'b> {
    #[allow(clippy::unwrap_used)]
    fn acknowledge(&mut self, id: ProposalId, pointer: ProposalPointer, proposal: Proposal) {
        self.state
            .proposals
            .register(id.into(), (proposal, pointer), None, None)
            .unwrap_or_default(); // Can't happen as by construction key is unique
    }
}

impl<'b> WitnessSlice for DefaultValidationContext<'b> {
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

    fn required_signers(&mut self) -> BTreeSet<Hash<28>> {
        mem::take(&mut self.required_signers)
    }

    fn required_bootstrap_signers(&mut self) -> BTreeSet<Hash<28>> {
        mem::take(&mut self.required_bootstrap_signers)
    }
}
