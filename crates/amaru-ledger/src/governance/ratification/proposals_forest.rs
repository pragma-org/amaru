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

use super::{
    proposals_tree::{ProposalsInsertError, ProposalsTree, Sibling},
    CommitteeUpdate, OrphanProposal, ProposalEnum,
};
use amaru_kernel::{
    display_protocol_parameters_update, expect_stake_credential, ComparableProposalId,
    Constitution, Epoch, GovAction, Nullable, ProposalId, ProtocolParamUpdate, ProtocolVersion,
};
use std::{
    collections::{BTreeMap, VecDeque},
    fmt,
    rc::Rc,
};

#[derive(Debug)]
pub struct ProposalsForest {
    /// We keep a map of id -> ProposalEnum. This serves as a lookup table to retrieve proposals
    /// from the forest in a timely manner while the relationships between all proposals is
    /// maintained independently.
    proposals: BTreeMap<Rc<ComparableProposalId>, ProposalEnum>,

    /// The order in which proposals are inserted matters. The forest is an insertion-preserving
    /// structure. Iterating on the forest will yield the proposals in the order they were
    /// inserted.
    sequence: VecDeque<Rc<ComparableProposalId>>,

    // Finally, the relation between proposals is preserved through multiple tree-like structures.
    // This is what gives this data-structure its name.
    protocol_parameters: ProposalsTree,
    hard_fork: ProposalsTree,
    constitutional_committee: ProposalsTree,
    constitution: ProposalsTree,
}

impl ProposalsForest {
    pub fn empty() -> Self {
        ProposalsForest {
            proposals: BTreeMap::new(),
            sequence: VecDeque::new(),
            protocol_parameters: ProposalsTree::Empty,
            hard_fork: ProposalsTree::Empty,
            constitutional_committee: ProposalsTree::Empty,
            constitution: ProposalsTree::Empty,
        }
    }

    /// Returns an iterator over the forest's proposal.
    pub fn iter(&self) -> impl Iterator<Item = (&ComparableProposalId, &ProposalEnum)> {
        ProposalsForestIterator::new(self)
    }

    /// Insert a proposal in the forest. This retains the order of insertion, so it is assumed
    /// that:
    ///
    /// 1. The caller has taken care of ordering proposals so that when a proposal has a parent
    ///    relationship with another, that other has been inserted before.
    ///
    /// 2. Except from the first proposal at the root of the tree, there's no proposal referring to
    ///    a non-existing parent (which is vaguely similar to the first point).
    ///
    /// If these two conditions are respected, then `insert` cannot fail and will always yield
    /// `Ok`.
    pub fn insert(
        &mut self,
        id: ComparableProposalId,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError> {
        use amaru_kernel::GovAction::*;

        let id = Rc::new(id);

        self.sequence.push_back(id.clone());

        match proposal {
            ParameterChange(parent, update, _guardrails_script) => {
                let parent = into_parent_id(parent);

                self.protocol_parameters
                    .insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id, ProposalEnum::ProtocolParameters(*update, parent));

                Ok(())
            }

            HardForkInitiation(parent, protocol_version) => {
                let parent = into_parent_id(parent);

                self.hard_fork.insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id, ProposalEnum::HardFork(protocol_version, parent));

                Ok(())
            }

            TreasuryWithdrawals(withdrawals, guardrails_script) => {
                let withdrawals = withdrawals.to_vec().into_iter().fold(
                    BTreeMap::new(),
                    |mut accum, (reward_account, amount)| {
                        accum.insert(expect_stake_credential(&reward_account), amount);
                        accum
                    },
                );

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal {
                        withdrawals,
                        guardrails: Option::from(guardrails_script),
                    }),
                );

                Ok(())
            }

            UpdateCommittee(parent, removed, added, threshold) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::ConstitutionalCommittee(
                        CommitteeUpdate::ChangeMembers {
                            removed: removed.to_vec().into_iter().collect(),
                            added: added
                                .to_vec()
                                .into_iter()
                                .map(|(k, v)| (k, Epoch::from(v)))
                                .collect(),
                            threshold,
                        },
                        parent,
                    ),
                );

                Ok(())
            }

            NoConfidence(parent) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, parent),
                );

                Ok(())
            }

            NewConstitution(parent, constitution) => {
                let parent = into_parent_id(parent);

                self.constitution.insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id.clone(), ProposalEnum::Constitution(constitution, parent));

                Ok(())
            }

            Information => {
                self.proposals
                    .insert(id.clone(), ProposalEnum::Orphan(OrphanProposal::NicePoll));

                Ok(())
            }
        }
    }
}

/// An iterator to conveniently navigate a forest in proposals' order.
#[derive(Debug)]
pub struct ProposalsForestIterator<'a> {
    cursor: usize,
    forest: &'a ProposalsForest,
}

impl<'a> ProposalsForestIterator<'a> {
    pub fn new(forest: &'a ProposalsForest) -> Self {
        Self { cursor: 0, forest }
    }
}

impl<'a> Iterator for ProposalsForestIterator<'a> {
    type Item = (&'a ComparableProposalId, &'a ProposalEnum);

    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor >= self.forest.sequence.len() {
            return None;
        }

        let id = self.forest.sequence.get(self.cursor)?;
        let proposal = self.forest.proposals.get(id)?;

        self.cursor += 1;

        Some((id, proposal))
    }
}

/// Pretty-print a forest. Proposals are shown by groups, and in order *within each group*. The
/// total ordering is however lost in this representation.
///
/// For example:
///
/// ```no_run
/// Protocol Parameter Updates
/// └─ 0.f6cb185a1f:
///    │ · min_fee_b=42
///    │ · max_block_ex_units={mem=300000, cpu=30000}
///    ├─ 0.27997e2a0b:
///    │    · key_deposit=5000000
///    └─ 0.71762f767d:
///         · key_deposit=1234567
///
/// Hard forks
/// └─ 0.19a065f326: version=10.0
///
/// Others
/// ├─ nice poll
/// ├─ withdrawal=1₳
/// └─ withdrawal=300000₳
/// ```
impl fmt::Display for ProposalsForest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ---- Renderers -----------------------------------------------------

        // Prints a section header + the tree underneath.
        fn section<'a, A>(
            f: &mut fmt::Formatter<'_>,
            title: &str,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            tree: &'a ProposalsTree,
        ) -> fmt::Result {
            if matches!(tree, ProposalsTree::Empty) {
                return Ok(());
            }
            writeln!(f, "{title}")?;
            render_tree(f, lookup, summarize, tree)?;
            Ok(())
        }

        // Renders a whole tree (which is just a bag of siblings at each Node).
        fn render_tree<'a, A>(
            f: &mut fmt::Formatter<'_>,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            tree: &'a ProposalsTree,
        ) -> fmt::Result {
            match tree {
                ProposalsTree::Empty => Ok(()),
                ProposalsTree::Node { siblings, .. } => {
                    for (i, s) in siblings.iter().enumerate() {
                        let is_last = i + 1 == siblings.len();
                        render_sibling(f, s, "", lookup.clone(), summarize.clone(), is_last)?;
                    }
                    Ok(())
                }
            }
        }

        // Render a single sibling + its (flattened) children.
        fn render_sibling<'a, A>(
            f: &mut fmt::Formatter<'_>,
            s: &'a Sibling,
            prefix: &str,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            is_last: bool,
        ) -> fmt::Result {
            let branch = if is_last { "└─" } else { "├─" };

            // Children are a Vec<ProposalsTree<A>>; flatten to a linear list of Sibling<A>
            // to get correct "last" detection for drawing.
            let flat: Vec<&Sibling> = collect_child_siblings(&s.children);
            let next_prefix = if is_last {
                format!("{prefix}   ")
            } else {
                format!("{prefix}│  ")
            };

            writeln!(
                f,
                "{prefix}{branch} {}: {}",
                s.id.to_string().chars().take(12).collect::<String>(),
                match lookup(&s.id) {
                    None => "?".to_string(), // NOTE: should be impossible on a well-formed forest.
                    Some(a) => summarize(
                        a,
                        &if is_last && flat.is_empty() {
                            format!(" {prefix}    ")
                        } else if is_last {
                            format!(" {prefix}  │ ")
                        } else if flat.is_empty() {
                            format!("│{prefix}    ")
                        } else {
                            format!("│{prefix}  │ ")
                        }
                    )?,
                }
            )?;

            for (idx, cs) in flat.iter().enumerate() {
                let last_here = idx + 1 == flat.len();
                render_sibling(
                    f,
                    cs,
                    &next_prefix,
                    lookup.clone(),
                    summarize.clone(),
                    last_here,
                )?;
            }
            Ok(())
        }

        // Gather all siblings from all non-empty child subtrees, in order.
        fn collect_child_siblings(children: &[ProposalsTree]) -> Vec<&Sibling> {
            let mut out = Vec::new();
            for c in children {
                if let ProposalsTree::Node { siblings, .. } = c {
                    for s in siblings {
                        out.push(s);
                    }
                }
            }
            out
        }

        // ---- Forest printing -----------------------------------------------

        if !self.sequence.is_empty() {
            writeln!(f)?;
        } else {
            write!(f, "empty")?;
        }

        section::<ProtocolParamUpdate>(
            f,
            "Protocol Parameter Updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::ProtocolParameters(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|pp, prefix| {
                Ok(format!(
                    "\n{}",
                    display_protocol_parameters_update(pp, &format!("{prefix}· "))?
                ))
            }),
            &self.protocol_parameters,
        )?;

        section::<ProtocolVersion>(
            f,
            "Hard forks",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::HardFork(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|protocol_version, _| {
                Ok(format!(
                    "version={}.{}",
                    protocol_version.0, protocol_version.1
                ))
            }),
            &self.hard_fork,
        )?;

        section::<CommitteeUpdate>(
            f,
            "Constitutional Committee Updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::ConstitutionalCommittee(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|committee_update, _| Ok(committee_update.to_string())),
            &self.constitutional_committee,
        )?;

        section::<Constitution>(
            f,
            "Constitution updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::Constitution(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|constitution, _| {
                Ok(format!(
                    "{} with {}",
                    constitution.anchor.url,
                    match constitution.guardrail_script {
                        Nullable::Some(hash) => format!(
                            "guardrails={}",
                            hash.to_string().chars().take(8).collect::<String>()
                        ),
                        Nullable::Undefined | Nullable::Null => "no guardrails".to_string(),
                    },
                ))
            }),
            &self.constitution,
        )?;

        let others = self
            .sequence
            .iter()
            .filter_map(|id| match self.proposals.get(id) {
                Some(ProposalEnum::Orphan(o)) => Some(o),
                _ => None,
            })
            .collect::<Vec<_>>();

        // Others, represented as a flat sequence.
        if !others.is_empty() {
            writeln!(f, "Others")?;
            for (i, o) in others.iter().enumerate() {
                let is_last = i + 1 == others.len();
                let branch = if is_last { "└─" } else { "├─" };
                writeln!(f, "{branch} {o}")?;
            }
        }

        Ok(())
    }
}

// Helpers
// ----------------------------------------------------------------------------

fn into_parent_id(nullable: Nullable<ProposalId>) -> Option<Rc<ComparableProposalId>> {
    match nullable {
        Nullable::Undefined | Nullable::Null => None,
        Nullable::Some(id) => Some(Rc::new(ComparableProposalId::from(id))),
    }
}
