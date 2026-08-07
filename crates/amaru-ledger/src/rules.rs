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

use std::{borrow::Cow, fmt, fmt::Display};

use amaru_kernel::{
    Block, Certificate, GovernanceAction, HasOwnership, Proposal, TransactionBody, Voter, parse_reward_account,
};
use amaru_observability::debug_span;
pub use block::execute as validate_block;

use crate::context::PreparationContext;

pub mod block;
pub mod transaction;

#[derive(Debug)]
pub enum TransactionField {
    Withdrawals,
}

#[derive(Debug)]
pub struct WithPosition<T: Display> {
    pub position: usize,
    pub element: T,
}

impl<T: Display> Display for WithPosition<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "#{}: {}", self.position, self.element)
    }
}

/// Prepare the context for a whole block of transactions.
pub fn prepare_block<'a>(context: &mut impl PreparationContext<'a>, block: &'a Block) {
    debug_span!(ledger::block::PREPARE)
        .in_scope(|| block.transaction_bodies.iter().for_each(|transaction| prepare_transaction(context, transaction)));
}

/// Prepare the context for a single transaction.
pub fn prepare_transaction<'a>(context: &mut impl PreparationContext<'a>, transaction: &'a TransactionBody) {
    prepare_inputs(context, transaction);
    prepare_withdrawals(context, transaction);

    let certificates = transaction.certificates.as_deref().unwrap_or(&[]).iter();
    certificates.for_each(|certificate| prepare_certificate(context, certificate));

    prepare_votes(context, transaction);

    let gov_actions = transaction.proposals.as_deref().unwrap_or(&[]).iter();
    gov_actions.for_each(|action| prepare_governance_action(context, action));
}

/// Collect and require the inputs from a single transaction.
fn prepare_inputs<'a>(context: &mut impl PreparationContext<'a>, transaction: &'a TransactionBody) {
    let inputs = transaction.inputs.iter();
    let collaterals = transaction.collateral.as_deref().unwrap_or(&[]).iter();
    let reference_inputs = transaction.reference_inputs.as_deref().unwrap_or(&[]).iter();
    inputs.chain(reference_inputs).chain(collaterals).for_each(|input| context.require_input(input));
}

/// Collect and require the reward accounts referenced by a transaction's withdrawals.
fn prepare_withdrawals<'a>(context: &mut impl PreparationContext<'a>, transaction: &'a TransactionBody) {
    let Some(withdrawals) = transaction.withdrawals.as_ref() else {
        return;
    };
    withdrawals
        .iter()
        .filter_map(|(bytes, _)| parse_reward_account(bytes))
        .for_each(|(account, _)| context.require_account(Cow::Owned(account)));
}

/// Collect and require the proposals a transaction votes on, along with the state each voter's
/// existence is decided against. A `Voter` carries a bare hash rather than a credential, so the
/// credential is rebuilt here rather than borrowed from the transaction.
fn prepare_votes<'a>(context: &mut impl PreparationContext<'a>, transaction: &'a TransactionBody) {
    let Some(votes) = transaction.votes.as_ref() else {
        return;
    };

    for (voter, ballots) in votes.iter() {
        for (proposal_id, _) in ballots.iter() {
            context.require_proposal(proposal_id);
        }

        match voter {
            Voter::ConstitutionalCommitteeKey(_) | Voter::ConstitutionalCommitteeScript(_) => {
                context.require_committee_voter(voter.owner())
            }
            Voter::DRepKey(_) | Voter::DRepScript(_) => context.require_drep(Cow::Owned(voter.owner())),
            Voter::StakePoolKey(pool) => context.require_pool(pool),
        };
    }
}

fn prepare_governance_action<'a>(context: &mut impl PreparationContext<'a>, proposal: &'a Proposal) {
    if let Some((account, _)) = parse_reward_account(&proposal.reward_account) {
        context.require_account(Cow::Owned(account));
    }

    if let GovernanceAction::TreasuryWithdrawals(withdrawals, _) = &proposal.gov_action {
        withdrawals
            .iter()
            .filter_map(|withdrawal| parse_reward_account(&withdrawal.0))
            .for_each(|(account, _)| context.require_account(Cow::Owned(account)));
    }
}
/// Collect and require values from a single certificate.
fn prepare_certificate<'a>(context: &mut impl PreparationContext<'a>, certificate: &'a Certificate) {
    match certificate {
        Certificate::StakeDelegation(credential, pool_key_hash)
        | Certificate::StakeRegDeleg(credential, pool_key_hash, _) => {
            context.require_account(Cow::Borrowed(credential));
            context.require_pool(pool_key_hash);
        }

        Certificate::PoolRegistration(params) => context.require_pool(&params.id),
        Certificate::PoolRetirement(pool_id, _) => context.require_pool(pool_id),

        Certificate::StakeVoteDeleg(credential, pool_key_hash, drep)
        | Certificate::StakeVoteRegDeleg(credential, pool_key_hash, drep, _) => {
            context.require_account(Cow::Borrowed(credential));
            context.require_pool(pool_key_hash);
            context.require_drep_delegation(drep);
        }

        Certificate::VoteRegDeleg(credential, drep, _) | Certificate::VoteDeleg(credential, drep) => {
            context.require_account(Cow::Borrowed(credential));
            context.require_drep_delegation(drep);
        }
        Certificate::AuthCommitteeHot(cold_credential, _) | Certificate::ResignCommitteeCold(cold_credential, _) => {
            context.require_committee_member(cold_credential)
        }

        Certificate::RegDRepCert(drep, _, _)
        | Certificate::UnRegDRepCert(drep, _)
        | Certificate::UpdateDRepCert(drep, _) => context.require_drep(Cow::Borrowed(drep)),

        Certificate::StakeRegistration(credential)
        | Certificate::Reg(credential, _)
        | Certificate::UnReg(credential, _)
        | Certificate::StakeDeregistration(credential) => context.require_account(Cow::Borrowed(credential)),
    };
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        sync::LazyLock,
    };

    use amaru_kernel::{
        DRep, Hash, MemoizedTransactionOutput, NetworkName, PREPROD_DEFAULT_PROTOCOL_PARAMETERS, PREPROD_ERA_HISTORY,
        PREPROD_GLOBAL_PARAMETERS, ProtocolParameters, StakeCredential, TransactionInput,
        cardano::network_block::CONWAY_BLOCK, cbor, hash,
    };
    use amaru_plutus::arena_pool::ArenaPool;

    use super::*;
    use crate::{
        context::{DefaultPreparationContext, DefaultValidationContext},
        epoch_transition::GovernanceActivity,
        rules::block::{BlockValidation, InvalidBlockDetails},
        tests::{fake_input, fake_output},
    };

    static MODIFIED_CONWAY_BLOCK: LazyLock<Vec<u8>> = LazyLock::new(|| {
        // These bytes are modified to be invalid CBOR, originally from Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
        hex::decode("830785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex")
    });

    /// The two inputs the Conway block spends, resolved so that block-level rules which consult
    /// the utxo (reference-script size) can run.
    fn conway_block_utxo() -> BTreeMap<TransactionInput, MemoizedTransactionOutput> {
        BTreeMap::from([
            (
                fake_input("2e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf4", 0),
                fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
            ),
            (
                fake_input("d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb", 0),
                fake_output("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335"),
            ),
        ])
    }

    static ARENA_POOL: LazyLock<ArenaPool> = LazyLock::new(|| ArenaPool::new(10, 1_024_000));

    #[test]
    fn validate_block_serialization_err() {
        assert!(parse_block(&MODIFIED_CONWAY_BLOCK).is_err())
    }

    /// `max_block_header_size` of 1 rejects the block on the first block-level rule, before any
    /// transaction is looked at.
    #[test]
    fn validate_block_header_size_too_big() {
        let pp = ProtocolParameters { max_block_header_size: 1, ..PREPROD_DEFAULT_PROTOCOL_PARAMETERS.clone() };

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        let results = block::execute(
            &mut DefaultValidationContext::default(),
            &ARENA_POOL,
            NetworkName::Preprod,
            &pp,
            &PREPROD_ERA_HISTORY,
            &PREPROD_GLOBAL_PARAMETERS,
            GovernanceActivity { consecutive_dormant_epochs: 0 },
            block,
        );

        assert!(matches!(results, BlockValidation::Invalid(_, _, InvalidBlockDetails::HeaderSizeTooBig { .. })))
    }

    /// A block walks every transaction body, so preparing it requires each spent input.
    #[test]
    fn prepare_block_requires_every_spent_input() {
        let mut context = DefaultPreparationContext::new();

        let block = parse_block(&CONWAY_BLOCK).unwrap();

        prepare_block(&mut context, &block);

        assert_eq!(
            context.utxo.into_iter().cloned().collect::<BTreeSet<_>>(),
            conway_block_utxo().into_keys().collect()
        );
    }

    /// A body carrying every kind of reference the preparation pass collects: spent, reference and
    /// collateral inputs, a withdrawal, one certificate per `require_*` target, a vote, and a
    /// treasury-withdrawal proposal whose return account must also be resolved.
    const PREPARE_TRANSACTION_BODY: &str = "a900d9010281825820a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1000180021a00030d400dd9010281825820a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a3a30012d9010281825820a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a20005a1581de093c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd81a000f4240048782008200581c93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd883028200581c93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd8581c111111111111111111111111111111111111111111111111111111118304581c11111111111111111111111111111111111111111111111111111111183283098200581c93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd88200581c22222222222222222222222222222222222222222222222222222222840a8200581c93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd8581c111111111111111111111111111111111111111111111111111111118200581c22222222222222222222222222222222222222222222222222222222830e8200581c333333333333333333333333333333333333333333333333333333338200581c4444444444444444444444444444444444444444444444444444444484108200581c222222222222222222222222222222222222222222222222222222221a1dcd6500f613a38202581c22222222222222222222222222222222222222222222222222222222a1825820b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1008201f68200581c55555555555555555555555555555555555555555555555555555555a1825820b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1008201f68204581c66666666666666666666666666666666666666666666666666666666a1825820b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1b1008201f614d9010281841b000000174876e800581de093c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd88302a1581de0222222222222222222222222222222222222222222222222222222221a006acfc0f6827368747470733a2f2f6578616d706c652e636f6d58200000000000000000000000000000000000000000000000000000000000000000";

    #[test]
    fn prepare_transaction_requires_every_referenced_entity() {
        let bytes = hex::decode(PREPARE_TRANSACTION_BODY).expect("invalid body hex");
        let transaction: TransactionBody = cbor::decode(&bytes).expect("failed to decode body");

        let mut context = DefaultPreparationContext::new();
        prepare_transaction(&mut context, &transaction);

        let dev_key = hash!("93c191b1094746961f6f00fba27f3d8eff6a66490baf806d4e179fd8");
        let proposal_key = hash!("22222222222222222222222222222222222222222222222222222222");

        let pool = Hash::from(hex::decode("11".repeat(28)).unwrap().as_slice());
        let drep = Hash::from(hex::decode("22".repeat(28)).unwrap().as_slice());
        let cc_cold = Hash::from(hex::decode("33".repeat(28)).unwrap().as_slice());
        let cc_hot = Hash::from(hex::decode("55".repeat(28)).unwrap().as_slice());
        let voting_pool = Hash::from(hex::decode("66".repeat(28)).unwrap().as_slice());

        // spent, collateral and reference inputs all have to be resolvable
        assert_eq!(
            context.utxo.into_iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([
                fake_input(&"a1".repeat(32), 0),
                fake_input(&"a2".repeat(32), 0),
                fake_input(&"a3".repeat(32), 0),
            ])
        );

        assert_eq!(
            context.accounts.into_iter().map(|k| k.into_owned()).collect::<BTreeSet<_>>(),
            // the withdrawal's reward account, plus the proposal's treasury-withdrawal target
            BTreeSet::from([StakeCredential::AddrKeyhash(proposal_key), StakeCredential::AddrKeyhash(dev_key),])
        );
        // the certificates' pool, plus the one that cast a vote
        assert_eq!(context.pools.into_iter().copied().collect::<BTreeSet<_>>(), BTreeSet::from([pool, voting_pool]));
        // the certificates' DRep, which is also the one that cast a vote
        assert_eq!(
            context.dreps.into_iter().map(|k| k.into_owned()).collect::<BTreeSet<_>>(),
            BTreeSet::from([StakeCredential::AddrKeyhash(drep)])
        );
        assert_eq!(context.drep_delegations.into_iter().cloned().collect::<Vec<_>>(), vec![DRep::Key(drep)]);
        // a certificate names a member by cold credential...
        assert_eq!(
            context.committee.into_iter().cloned().collect::<BTreeSet<_>>(),
            BTreeSet::from([StakeCredential::AddrKeyhash(cc_cold)])
        );
        // ...whereas a vote names one by the hot credential it authorized
        assert_eq!(context.committee_voters, BTreeSet::from([StakeCredential::AddrKeyhash(cc_hot)]));

        // the single proposal all three votes are cast on
        assert_eq!(context.proposals.len(), 1);
    }

    fn parse_block(bytes: &[u8]) -> Result<Block, cbor::decode::Error> {
        let (_, block): (u16, Block) = cbor::decode(bytes)?;
        Ok(block)
    }
}
