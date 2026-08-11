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

use amaru_kernel::{ORIGIN_HASH, Point};
use amaru_observability::{debug, info, info_record};
use amaru_ouroboros::ChainStore;
use anyhow::bail;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearValidity {
    /// Startup path: only clear `valid=true` after the tip (volatile ledger is rebuilt).
    /// Invalid flags are kept so previously rejected blocks stay skipped.
    ValidOnly,
    /// Offline recovery: clear both valid and invalid flags on all descendants so blocks can be
    /// re-validated after a false invalid or a deeper rewind.
    All,
}

/// Realign the chain store so the best chain ends at `tip`, the anchor is `tip`, and validation
/// flags after `tip` are cleared according to `clear`.
///
/// Headers and blocks are retained. When a best chain already exists, `tip` must lie on it;
/// otherwise the store is left untouched and an error is returned.
pub fn realign_chain_store_to(chain_store: &dyn ChainStore, tip: Point, clear: ClearValidity) -> anyhow::Result<()> {
    info!(consensus::chain_db::INITIALIZE, ledger_tip = %tip);

    let best_chain_hash = chain_store.get_best_chain_hash();
    let has_best_chain = best_chain_hash != ORIGIN_HASH;

    // Every fork branches at or after the immutable tip, which cannot be rolled back, so the
    // ledger tip is always on the recorded best chain. When it is not, the two databases describe
    // different chains and truncating the best chain would silently discard headers. This is
    // checked before any mutation so that a rejected chain database is left untouched.
    if has_best_chain && chain_store.load_from_best_chain(&tip).is_none() {
        bail!(
            "the chain database is inconsistent with the ledger: its best chain, ending at \
             {best_chain_hash}, does not contain the ledger tip {tip}. This happens when \
             a ledger snapshot is imported on top of a chain database built for another chain. \
             Remove the chain database so that it can be rebuilt from the ledger tip."
        );
    }

    chain_store.set_anchor_hash(&tip.hash())?;
    chain_store.set_block_valid(&tip.hash(), true)?;
    if has_best_chain {
        chain_store.switch_to_fork(&tip, &[])?;
    } else {
        chain_store.roll_forward_chain(&tip)?;
    }

    info_record!(consensus::chain_db::INITIALIZE, best_chain_hash = best_chain_hash);
    clear_validation_after_tip(chain_store, tip, clear)?;
    Ok(())
}

fn clear_validation_after_tip(chain_store: &dyn ChainStore, tip: Point, clear: ClearValidity) -> anyhow::Result<()> {
    let mut to_visit = chain_store.get_children(&tip.hash());
    let mut count = 0;

    while let Some(hash) = to_visit.pop() {
        let Some((_header, validity)) = chain_store.load_header_with_validity(&hash) else {
            continue;
        };

        let should_clear =
            matches!((clear, validity), (ClearValidity::ValidOnly, Some(true)) | (ClearValidity::All, Some(_)));

        if should_clear {
            count += 1;
            chain_store.remove_block_valid(&hash)?;
        }

        to_visit.extend(chain_store.get_children(&hash));
    }
    debug!(consensus::chain_db::CLEAR_VALID_DESCENDANTS, count = count);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use amaru_kernel::{BlockHeader, IsHeader, make_header};
    use amaru_ouroboros::{BaseReadChainStore, WriteChainStore, in_memory_chain_store::InMemoryChainStore};

    use super::*;

    #[test]
    fn realign_valid_only_keeps_invalid_flags() {
        // h0 -- h1 -- h2 -- h3   the chain the consensus had validated
        //         \
        //          h2a          a fork that was validated and rejected
        let h0 = header(1, 1, None);
        let h1 = header(2, 2, Some(&h0));
        let h2 = header(3, 3, Some(&h1));
        let h3 = header(4, 4, Some(&h2));
        let h2a = header(3, 30, Some(&h1));

        let chain_store = Arc::new(InMemoryChainStore::new());
        for header in [&h0, &h1, &h2, &h3, &h2a] {
            chain_store.store_header(header).unwrap();
            chain_store.set_block_valid(&header.hash(), true).unwrap();
        }
        chain_store.set_block_valid(&h2a.hash(), false).unwrap();
        for header in [&h0, &h1, &h2, &h3] {
            chain_store.roll_forward_chain(&header.point()).unwrap();
        }
        chain_store.set_anchor_hash(&h0.hash()).unwrap();

        realign_chain_store_to(chain_store.as_ref(), h1.point(), ClearValidity::ValidOnly).unwrap();

        assert_eq!(chain_store.get_anchor_hash(), h1.hash(), "the anchor must move to the ledger tip");
        assert_eq!(chain_store.get_best_chain_hash(), h1.hash(), "the best chain must end at the ledger tip");
        assert!(chain_store.load_from_best_chain(&h1.point()).is_some(), "the ledger tip stays on the best chain");
        assert!(chain_store.load_from_best_chain(&h2.point()).is_none(), "h2 must leave the best chain");
        assert!(chain_store.load_from_best_chain(&h3.point()).is_none(), "h3 must leave the best chain");

        assert_eq!(validity(chain_store.as_ref(), &h0), Some(true), "blocks before the ledger tip stay validated");
        assert_eq!(validity(chain_store.as_ref(), &h1), Some(true), "the ledger tip stays validated");
        assert_eq!(validity(chain_store.as_ref(), &h2), None, "h2 must be applied to the ledger again");
        assert_eq!(validity(chain_store.as_ref(), &h3), None, "h3 must be applied to the ledger again");
        assert_eq!(validity(chain_store.as_ref(), &h2a), Some(false), "an invalid block was never applied");
    }

    #[test]
    fn realign_all_clears_invalid_flags_on_descendants() {
        let h0 = header(1, 1, None);
        let h1 = header(2, 2, Some(&h0));
        let h2 = header(3, 3, Some(&h1));
        let h3 = header(4, 4, Some(&h2));
        let h2a = header(3, 30, Some(&h1));

        let chain_store = Arc::new(InMemoryChainStore::new());
        for header in [&h0, &h1, &h2, &h3, &h2a] {
            chain_store.store_header(header).unwrap();
            chain_store.set_block_valid(&header.hash(), true).unwrap();
        }
        chain_store.set_block_valid(&h2a.hash(), false).unwrap();
        for header in [&h0, &h1, &h2, &h3] {
            chain_store.roll_forward_chain(&header.point()).unwrap();
        }
        chain_store.set_anchor_hash(&h0.hash()).unwrap();

        realign_chain_store_to(chain_store.as_ref(), h1.point(), ClearValidity::All).unwrap();

        assert_eq!(chain_store.get_anchor_hash(), h1.hash());
        assert_eq!(chain_store.get_best_chain_hash(), h1.hash());
        assert!(chain_store.load_from_best_chain(&h2.point()).is_none());
        assert_eq!(validity(chain_store.as_ref(), &h0), Some(true));
        assert_eq!(validity(chain_store.as_ref(), &h1), Some(true));
        assert_eq!(validity(chain_store.as_ref(), &h2), None);
        assert_eq!(validity(chain_store.as_ref(), &h3), None);
        assert_eq!(
            validity(chain_store.as_ref(), &h2a),
            None,
            "invalid flags after the tip must be cleared for recovery"
        );
    }

    #[test]
    fn start_the_best_chain_on_a_store_that_has_none() {
        let h0 = header(1, 1, None);

        let chain_store = Arc::new(InMemoryChainStore::new());
        chain_store.store_header(&h0).unwrap();

        realign_chain_store_to(chain_store.as_ref(), h0.point(), ClearValidity::ValidOnly).unwrap();

        assert_eq!(chain_store.get_anchor_hash(), h0.hash());
        assert_eq!(chain_store.get_best_chain_hash(), h0.hash());
        assert!(chain_store.load_from_best_chain(&h0.point()).is_some(), "the ledger tip starts the best chain");
        assert_eq!(validity(chain_store.as_ref(), &h0), Some(true));
    }

    #[test]
    fn reject_a_chain_store_that_does_not_contain_the_ledger_tip() {
        // h0 -- h1     the chain the consensus had built
        //   \
        //    h1a       the branch the ledger was bootstrapped on
        let h0 = header(1, 1, None);
        let h1 = header(2, 2, Some(&h0));
        let h1a = header(2, 20, Some(&h0));

        let chain_store = Arc::new(InMemoryChainStore::new());
        for header in [&h0, &h1, &h1a] {
            chain_store.store_header(header).unwrap();
        }
        for header in [&h0, &h1] {
            chain_store.roll_forward_chain(&header.point()).unwrap();
        }
        chain_store.set_anchor_hash(&h0.hash()).unwrap();

        let error =
            realign_chain_store_to(chain_store.as_ref(), h1a.point(), ClearValidity::All).unwrap_err().to_string();

        assert!(error.contains("inconsistent with the ledger"), "unexpected error: {error}");
        assert_eq!(chain_store.get_best_chain_hash(), h1.hash(), "the best chain must be left untouched");
        assert_eq!(chain_store.get_anchor_hash(), h0.hash(), "the anchor must be left untouched");
        for header in [&h0, &h1, &h1a] {
            assert_eq!(validity(chain_store.as_ref(), header), None, "no block validity must be recorded");
        }
    }

    fn header(block_height: u64, slot: u64, parent: Option<&BlockHeader>) -> BlockHeader {
        BlockHeader::from(make_header(block_height, slot, parent.map(BlockHeader::hash)))
    }

    fn validity(chain_store: &dyn ChainStore, header: &BlockHeader) -> Option<bool> {
        chain_store.load_header_with_validity(&header.hash()).and_then(|(_, validity)| validity)
    }
}
