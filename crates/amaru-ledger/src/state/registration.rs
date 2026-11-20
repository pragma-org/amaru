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

//! Pure construction of unsigned, ADA-funded pool registration transactions.

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Address, Certificate, Certificates, Credential, Hash, Lovelace, MemoizedDatum, MemoizedTransactionOutput, Network,
    NonEmptyVec, Point, PoolId, PoolParams, ProtocolParameters, Slot, Transaction, TransactionBody, TransactionInput,
    Value, VerificationKeyWitness, WitnessSet,
    cbor::{WithSize, to_cbor_with},
    from_cbor_no_leftovers_with,
    protocol_version::PROTOCOL_VERSION_12,
};
use thiserror::Error;

use self::RegistrationError::{InsufficientFunds, Overflow};

type Result<T> = std::result::Result<T, RegistrationError>;

/// Explicit inputs and registration identities to read together on the ledger worker.
#[derive(Debug, Clone)]
pub struct RegistrationQuery {
    pub inputs: BTreeSet<TransactionInput>,
    pub pool: PoolId,
    pub reward_account: Credential,
    /// Owner stake keys to inspect before registering and delegating them.
    pub owners: BTreeSet<Hash<28>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnerAction {
    RegisterAndDelegate,
    Delegate,
    Redelegate,
    Unchanged,
}

/// The certificates and deposit an owner needs at the captured ledger point.
#[derive(Debug, Clone)]
pub struct OwnerDelegation {
    pub action: OwnerAction,
    pub previous_pool: Option<PoolId>,
    pub registration_deposit: Lovelace,
}

/// An owned view at a single ledger point, including unpersisted adopted blocks.
#[derive(Debug, Clone)]
pub struct RegistrationContext {
    pub point: Point,
    pub network: Network,
    pub parameters: ProtocolParameters,
    /// Requested inputs that are unspent at `point`.
    pub inputs: BTreeMap<TransactionInput, MemoizedTransactionOutput>,
    pub pool: PoolId,
    pub pool_registered: bool,
    pub reward_account: Credential,
    pub reward_registered: bool,
    /// Includes every queried owner, including credentials that are not registered.
    pub owners: BTreeMap<Hash<28>, OwnerDelegation>,
}

#[derive(Debug, Error)]
pub enum RegistrationError {
    #[error("invalid pool registration: {0}")]
    Invalid(&'static str),
    #[error("funding input {0} is missing or already spent")]
    MissingInput(TransactionInput),
    #[error(
        "funding input {0} must be an ADA-only Shelley key output without a reference script on the selected network"
    )]
    UnsupportedInput(TransactionInput),
    #[error("insufficient funds for deposits, fee, and minimum change output")]
    InsufficientFunds,
    #[error("registration arithmetic overflow")]
    Overflow,
    #[error("registration CBOR is invalid")]
    Encoding(#[from] amaru_kernel::cbor::decode::Error),
}

/// Build an unsigned transaction, registering the reward credential when necessary.
///
/// Only explicit ADA-only key inputs and Shelley key change addresses are supported.
/// All funding, owner, cold, and newly registered reward keys are included in the fee
/// estimate, with duplicates counted once.
/// With `delegate_owners`, also register missing owner credentials and delegate owners
/// to this pool. The context must contain the state of every owner in that case.
/// The result contains no signatures; signing and submission belong to the caller.
pub fn build_registration(
    context: &RegistrationContext,
    inputs: &BTreeSet<TransactionInput>,
    pool: &PoolParams,
    change: &Address,
    expires: Slot,
    delegate_owners: bool,
) -> Result<Transaction> {
    context.validate(pool, change)?;
    check(!inputs.is_empty(), "at least one input is required")?;
    check(expires > context.point.slot_or_default(), "expiry must be after the captured tip")?;
    let (available, signers) = context.funding(inputs, pool)?;
    let (deposit, certificates) = context.certificates(pool, delegate_owners)?;
    let mut body = TransactionBody::default();
    body.inputs = inputs.iter().copied().collect();
    body.certificates = Some(certificates);
    body.validity_interval_start = Some(context.point.slot());
    body.validity_interval_end = Some(expires);
    body.network = Some(context.network);
    body.fee = context.parameters.min_fee_b;
    let tx =
        Transaction { body, witnesses: estimated_witnesses(signers), is_expected_valid: true, auxiliary_data: None };
    balance(tx, change, available.checked_sub(deposit).ok_or(InsufficientFunds)?, &context.parameters)
}

impl RegistrationContext {
    fn validate(&self, pool: &PoolParams, change: &Address) -> Result<()> {
        check(
            pool.id == self.pool && pool.reward_account.credential() == self.reward_account,
            "context does not match the pool and reward credential",
        )?;
        check(
            pool.reward_account.network() == self.network && change.network() == self.network,
            "reward and change addresses must match the selected network",
        )?;
        check(
            matches!(change, Address::Shelley(address) if !address.payment().is_script()),
            "only Shelley key change addresses are supported",
        )?;
        check(
            !pool.reward_account.is_script() && !pool.owners.is_empty(),
            "a key reward credential and at least one owner are required",
        )?;
        check(pool.cost >= self.parameters.min_pool_cost, "pool cost is below the protocol minimum")?;
        check(
            pool.metadata.as_ref().is_none_or(|metadata| metadata.content_hash.len() == 32),
            "metadata hash must contain 32 bytes",
        )
    }

    fn funding(&self, inputs: &BTreeSet<TransactionInput>, pool: &PoolParams) -> Result<(u64, usize)> {
        let mut signers = pool
            .owners
            .iter()
            .copied()
            .map(Credential::KeyHash)
            .chain([Credential::KeyHash(pool.id.into())])
            .chain((!self.reward_registered).then_some(self.reward_account))
            .collect::<BTreeSet<_>>();
        let mut total = 0u64;
        for input in inputs {
            let (key, coin) = self.funding_input(input)?;
            signers.insert(key);
            total = total.checked_add(coin).ok_or(Overflow)?;
        }
        Ok((total, signers.len()))
    }

    fn funding_input(&self, input: &TransactionInput) -> Result<(Credential, u64)> {
        let output = self.inputs.get(input).ok_or(RegistrationError::MissingInput(*input))?;
        match (&output.address, &output.value) {
            (Address::Shelley(address), Value::Coin(coin))
                if address.network() == self.network && !address.payment().is_script() && output.script.is_none() =>
            {
                Ok((*address.payment(), *coin))
            }
            _ => Err(RegistrationError::UnsupportedInput(*input)),
        }
    }

    fn certificates(&self, pool: &PoolParams, delegate_owners: bool) -> Result<(u64, Certificates)> {
        let params = &self.parameters;
        let mut register = BTreeMap::new();
        if !self.reward_registered {
            register.insert(self.reward_account, params.stake_credential_deposit);
        }
        let mut delegations = Vec::new();
        if delegate_owners {
            for owner in &pool.owners {
                let plan = self
                    .owners
                    .get(owner)
                    .ok_or(RegistrationError::Invalid("context is missing owner stake credentials"))?;
                let credential = Credential::KeyHash(*owner);
                if plan.action == OwnerAction::RegisterAndDelegate {
                    register.insert(credential, plan.registration_deposit);
                }
                if plan.action != OwnerAction::Unchanged {
                    delegations.push(Certificate::StakeDelegation(credential, pool.id));
                }
            }
        }
        let mut deposit = if self.pool_registered { 0 } else { params.stake_pool_deposit };
        let mut certificates = Vec::new();
        for (credential, amount) in register {
            deposit = deposit.checked_add(amount).ok_or(Overflow)?;
            certificates.push(Certificate::Reg(credential, amount));
        }
        certificates.push(Certificate::PoolRegistration(Box::new(pool.clone())));
        certificates.extend(delegations);
        let mut version = params.protocol_version;
        let certificates = from_cbor_no_leftovers_with(&to_cbor_with(&certificates, &mut version), &mut version)?;
        Ok((deposit, certificates))
    }
}

fn check(valid: bool, reason: &'static str) -> Result<()> {
    valid.then_some(()).ok_or(RegistrationError::Invalid(reason))
}

/// Fixed-size placeholders are used only for sizing and removed before returning the transaction.
fn estimated_witnesses(count: usize) -> WithSize<WitnessSet> {
    let witness = VerificationKeyWitness { verification_key: [0; 32].into(), signature: [0; 64].into() };
    WithSize::new(
        WitnessSet {
            verification_key_witness: Some(NonEmptyVec::new(witness.clone(), vec![witness; count - 1])),
            ..WitnessSet::default()
        },
        0,
    )
}

fn balance(mut tx: Transaction, change: &Address, available: u64, params: &ProtocolParameters) -> Result<Transaction> {
    let mut version = params.protocol_version;
    for _ in 0..16 {
        let coin = available.checked_sub(tx.body.fee).ok_or(InsufficientFunds)?;
        let output = MemoizedTransactionOutput::new(change.clone(), Value::Coin(coin), MemoizedDatum::None, None);
        let size = 160 + to_cbor_with(&output, &mut version).len() as u64;
        let minimum = size.checked_mul(params.lovelace_per_utxo_byte).ok_or(Overflow)?;
        tx.body.outputs = vec![(coin >= minimum).then_some(output).ok_or(InsufficientFunds)?];
        let fee = minimum_fee(&tx, params)?;
        if tx.body.fee >= fee {
            tx.witnesses = WithSize::new(WitnessSet::default(), 0);
            return Ok(from_cbor_no_leftovers_with(&to_cbor_with(&tx, &mut version), &mut version)?);
        }
        tx.body.fee = fee;
    }
    Err(RegistrationError::Invalid("fee calculation did not converge"))
}

fn minimum_fee(tx: &Transaction, params: &ProtocolParameters) -> Result<u64> {
    // External signers may emit the optional tag 258 on pre-v12 key witnesses.
    let tag = if params.protocol_version < PROTOCOL_VERSION_12 { 3 } else { 0 };
    let size = to_cbor_with(tx, &mut params.protocol_version.clone()).len() as u64 + tag;
    check(
        size <= u64::from(params.max_transaction_size),
        "signed transaction would exceed the maximum transaction size",
    )?;
    params.min_fee_a.checked_mul(size).and_then(|fee| fee.checked_add(params.min_fee_b)).ok_or(Overflow)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{
        BlockHeight, ByronAddress, CertificatePointer, Hash, Hasher, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS,
        PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, PROTOCOL_VERSION_11, RewardAccount, ShelleyAddress,
        TransactionPointer, UnitRationalNumber,
        ed25519::{Signer, SigningKey},
        protocol_version::PROTOCOL_VERSION_12,
    };
    use amaru_uplc::arena_pool::ArenaPool;

    use super::*;
    use crate::{
        context::{AccountState, DefaultValidationContext},
        epoch_transition::GovernanceActivity,
        state::volatile::{VolatileDB, VolatileFragment, VolatileSequence},
        store::{
            ReadStore,
            columns::{accounts, pools},
        },
    };

    fn fixture() -> (RegistrationContext, PoolParams, Address, Vec<SigningKey>) {
        let keys: Vec<_> = (1..=4).map(|tag| SigningKey::from_bytes(&[tag; 32])).collect();
        let hashes: Vec<_> = keys.iter().map(|key| Hasher::<224>::hash(key.verifying_key().as_bytes())).collect();
        let change = Address::Shelley(ShelleyAddress::new(Network::Testnet, Credential::KeyHash(hashes[0]), None));
        let pool = PoolParams {
            id: hashes[1].into(),
            vrf: Hash::new([0; 32]),
            pledge: 100_000_000,
            cost: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.min_pool_cost,
            margin: UnitRationalNumber::new(1, 100).unwrap(),
            reward_account: RewardAccount::new(Network::Testnet, Credential::KeyHash(hashes[2])),
            owners: [hashes[3]].into_iter().collect(),
            relays: vec![],
            metadata: None,
        };
        let input = TransactionInput { transaction_id: Hash::new([0; 32]), index: 0 };
        let context = RegistrationContext {
            point: Point::Specific(Slot::from(100_000_000), Hash::new([1; 32]), BlockHeight::from(10)),
            network: Network::Testnet,
            parameters: PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            inputs: BTreeMap::from([(
                input,
                MemoizedTransactionOutput::new(change.clone(), Value::Coin(1_000_000_000), MemoizedDatum::None, None),
            )]),
            pool: pool.id,
            pool_registered: false,
            reward_account: pool.reward_account.credential(),
            reward_registered: false,
            owners: BTreeMap::new(),
        };
        (context, pool, change, keys)
    }

    fn build(context: &RegistrationContext, pool: &PoolParams, change: &Address) -> Result<Transaction> {
        build_registration(
            context,
            &context.inputs.keys().copied().collect(),
            pool,
            change,
            context.point.slot().offset_by(3600),
            false,
        )
    }

    #[test_case::test_matrix(
        [PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, PROTOCOL_VERSION_11, PROTOCOL_VERSION_12],
        [false, true], [false, true]
    )]
    fn signed_registrations_pass_ledger_validation_and_charge_only_new_deposits(
        version: amaru_kernel::ProtocolVersion,
        pool_registered: bool,
        reward_registered: bool,
    ) {
        let (mut context, pool, change, keys) = fixture();
        context.parameters.protocol_version = version;
        context.pool_registered = pool_registered;
        context.reward_registered = reward_registered;
        let unsigned = build(&context, &pool, &change).unwrap();
        assert!(unsigned.witnesses.verification_key_witness.is_none());
        assert_eq!(unsigned, build(&context, &pool, &change).unwrap());
        let deposit = if pool_registered { 0 } else { context.parameters.stake_pool_deposit }
            + if reward_registered { 0 } else { context.parameters.stake_credential_deposit };
        assert_eq!(unsigned.body.outputs[0].value, Value::Coin(1_000_000_000 - deposit - unsigned.body.fee));
        let keys = keys.iter().enumerate().filter_map(|(i, key)| (i != 2 || !reward_registered).then_some(key));
        validate_signed(&context, sign(unsigned, keys, version));
    }

    fn sign<'a>(
        tx: Transaction,
        keys: impl Iterator<Item = &'a SigningKey>,
        mut version: amaru_kernel::ProtocolVersion,
    ) -> Transaction {
        let witnesses: Vec<_> = keys
            .map(|key| VerificationKeyWitness {
                verification_key: (*key.verifying_key().as_bytes()).into(),
                signature: key.sign(tx.body.id().as_ref()).to_bytes().into(),
            })
            .collect();
        let witnesses =
            WitnessSet { verification_key_witness: Some(witnesses.try_into().unwrap()), ..WitnessSet::default() };
        let mut encoded = amaru_kernel::cbor::Encoder::new(Vec::new());
        encoded.array(4).unwrap().encode_with(&tx.body, &mut version).unwrap();
        encoded.encode_with(&witnesses, &mut PROTOCOL_VERSION_12.clone()).unwrap().bool(true).unwrap().null().unwrap();
        from_cbor_no_leftovers_with(&encoded.into_writer(), &mut version).unwrap()
    }

    fn validate_signed(context: &RegistrationContext, tx: Transaction) {
        let account =
            AccountState { deposit: context.parameters.stake_credential_deposit, pool: None, drep: None, rewards: 0 };
        let mut validation = DefaultValidationContext::new(
            context.inputs.clone(),
            context.pool_registered.then_some(context.pool).into_iter().collect(),
            context.reward_registered.then_some((context.reward_account, account)).into_iter().collect(),
            BTreeMap::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            Default::default(),
            0,
        );
        let network = NetworkName::Preprod;
        crate::rules::block::validate_transaction(
            &mut validation,
            &ArenaPool::new(1, 1024 * 1024),
            network,
            &context.parameters,
            network.as_era_history().unwrap(),
            network.as_global_parameters().unwrap(),
            GovernanceActivity::default(),
            None,
            TransactionPointer { slot: context.point.slot(), transaction_index: 0 },
            tx.tx_ref(),
        )
        .unwrap();
    }

    #[test]
    fn shared_owner_reward_and_payment_key_is_charged_once() {
        let (mut context, mut pool, change, _) = fixture();
        let Address::Shelley(address) = &change else { unreachable!() };
        let Credential::KeyHash(hash) = address.payment() else { unreachable!() };
        pool.owners = [*hash].into_iter().collect();
        pool.reward_account = RewardAccount::new(Network::Testnet, *address.payment());
        context.reward_account = *address.payment();
        let shared = build(&context, &pool, &change).unwrap();
        let (context, pool, change, _) = fixture();
        let distinct = build(&context, &pool, &change).unwrap();
        assert!(shared.body.fee < distinct.body.fee);
    }

    #[test]
    fn rejects_script_and_byron_change() {
        let (context, pool, _, _) = fixture();
        let script =
            Address::Shelley(ShelleyAddress::new(Network::Testnet, Credential::ScriptHash(Hash::new([9; 28])), None));
        let byron = Address::Byron(
            ByronAddress::from_base58("2cWKMJemoBaiqkR9D1YZ2xQ2BhVxzauukrsxm8ttZUrto1f7kr5J1tD9uhtEtTc9U4PuF").unwrap(),
        );
        for change in [script, byron] {
            assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::Invalid(_))));
        }
    }

    #[test]
    fn rejects_missing_input_insufficient_change_wrong_network_and_oversize() {
        let (mut context, pool, change, _) = fixture();
        let inputs = context.inputs.keys().copied().collect();
        let expiry = context.point.slot().offset_by(3600);
        let original = context.inputs.clone();
        context.inputs.clear();
        assert!(matches!(
            build_registration(&context, &inputs, &pool, &change, expiry, false),
            Err(RegistrationError::MissingInput(_))
        ));
        context.inputs = original;
        context.inputs.values_mut().next().unwrap().value = Value::Coin(context.parameters.stake_pool_deposit);
        assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::InsufficientFunds)));
        let (mut context, pool, change, _) = fixture();
        context.network = Network::Mainnet;
        assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::Invalid(_))));
        context.network = Network::Testnet;
        context.parameters.max_transaction_size = 1;
        assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::Invalid(_))));
    }

    #[test]
    fn rejects_script_funding_and_arithmetic_overflow() {
        let (mut context, pool, change, _) = fixture();
        context.inputs.values_mut().next().unwrap().address =
            Address::Shelley(ShelleyAddress::new(Network::Testnet, Credential::ScriptHash(Hash::new([0; 28])), None));
        assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::UnsupportedInput(_))));
        let (mut context, pool, change, _) = fixture();
        context.parameters.min_fee_a = u64::MAX;
        assert!(matches!(build(&context, &pool, &change), Err(RegistrationError::Overflow)));
    }

    struct Stable {
        input: TransactionInput,
        output: MemoizedTransactionOutput,
        pool: PoolParams,
    }
    impl ReadStore for Stable {
        fn utxo(&self, input: &TransactionInput) -> crate::store::Result<Option<MemoizedTransactionOutput>> {
            Ok((*input == self.input).then(|| self.output.clone()))
        }
        fn pool(&self, id: &PoolId) -> crate::store::Result<Option<pools::Row>> {
            Ok((*id == self.pool.id)
                .then(|| pools::Row::new(CertificatePointer::default(), 500_000_000, self.pool.clone())))
        }
        fn account(&self, credential: &Credential) -> crate::store::Result<Option<accounts::Row>> {
            Ok((*credential == self.pool.reward_account.credential()).then(accounts::Row::default))
        }
    }

    #[test]
    fn live_context_sees_fresh_funding_spends_registrations_and_rollback() {
        let (context, pool, _, _) = fixture();
        let (spent, output) = context.inputs.first_key_value().unwrap();
        let fresh = TransactionInput { index: 1, ..*spent };
        let stable = Stable { input: *spent, output: output.clone(), pool: pool.clone() };
        let new_pool = PoolParams { id: PoolId::new([9; 28]), ..pool.clone() };
        let query = RegistrationQuery {
            inputs: BTreeSet::from([*spent, fresh]),
            pool: new_pool.id,
            reward_account: context.reward_account,
            owners: BTreeSet::new(),
        };
        let mut volatile = VolatileDB::default();
        let previous = Point::Specific(Slot::from(99_999_999), Hash::new([2; 32]), BlockHeight::from(9));
        volatile.push_back(VolatileFragment::default().anchor(previous, pool.id));
        let read = |volatile: &VolatileDB| {
            RegistrationContext::read(context.point, context.network, &context.parameters, volatile, &stable, &query)
                .unwrap()
        };
        let before = read(&volatile);
        assert!(before.inputs.contains_key(spent));
        assert!(!before.pool_registered);
        assert!(before.reward_registered);
        let mut fragment = VolatileFragment::default();
        fragment.utxo.consume(*spent);
        fragment.utxo.produce(fresh, Arc::new(output.clone()));
        fragment.accounts.unregister(query.reward_account);
        fragment.pools.register(new_pool.id, Arc::new((new_pool, CertificatePointer::default(), 500_000_000)));
        volatile.push_back(fragment.anchor(context.point, pool.id));
        let after = read(&volatile);
        assert_eq!(after.point, context.point);
        assert!(!after.inputs.contains_key(spent));
        assert!(after.inputs.contains_key(&fresh));
        assert!(after.pool_registered);
        assert!(!after.reward_registered);
        volatile.rollback_to(&previous).unwrap();
        let rolled_back = read(&volatile);
        assert_eq!(rolled_back.inputs, before.inputs);
        assert_eq!(rolled_back.pool_registered, before.pool_registered);
        assert_eq!(rolled_back.reward_registered, before.reward_registered);
    }
}
