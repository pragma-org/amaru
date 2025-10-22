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

/// This benchmark generates a large random header tree and a long sequence of actions
/// (adding headers from random peers and rollbacks) to be executed on a `HeadersTree`.
/// It then measures the average time taken to execute each action.
///
/// Run with: `cargo bench --bench headers_tree --features="test-utils profiling telemetry"`
///
/// Note: profiling requires `pprof` to be installed and only works on Unix systems.
/// If you run with profiling enabled, a flamegraph file named 'headers-tree-flamegraph.svg'
/// will be generated in the current directory. You can open it with a web browser and check which
/// functions are taking the most time.
///
/// This way of profiling was chosen to be able to isolate exactly the code we want to benchmark
/// and not the generation of the tree and actions.
///
#[expect(clippy::unwrap_used)]
#[cfg(all(unix, feature = "profiling", feature = "test-utils"))]
fn main() {
    use amaru_consensus::consensus::headers_tree::HeadersTree;
    use amaru_consensus::consensus::headers_tree::data_generation::{
        execute_actions_on_tree, generate_random_walks,
    };
    use amaru_consensus::consensus::stages::select_chain::DEFAULT_MAXIMUM_FRAGMENT_LENGTH;
    use amaru_kernel::{BlockHeader, IsHeader};
    use amaru_ouroboros_traits::ChainStore;
    use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
    use pprof::{ProfilerGuardBuilder, flamegraph::Options};
    use std::fs::File;
    use std::sync::Arc;

    let profile = false;
    let in_memory = false;

    let seed = 42;

    let max_length = DEFAULT_MAXIMUM_FRAGMENT_LENGTH;
    // We generate a tree with a larger depth in order to force the tree to be pruned when
    // the maximum length is reached for the best chain.
    let depth = max_length + 200;

    // A more realistic bench would use around 200 peers but this would make the bench take a really
    // long time to run.
    let peers_nb = 10;

    // Create a large tree of headers and random actions to be executed on a HeadersTree
    // from the list of peers.
    let generated_tree = generate_tree_of_headers(depth, seed);
    let tree = generated_tree.tree();
    assert!(
        tree.nodes().len() > 5000,
        "there are {} nodes",
        tree.nodes().len()
    );

    let generated_actions = generate_random_walks(&generated_tree, peers_nb);
    let actions = generated_actions.actions();
    assert!(actions.len() > 10000);

    // Initialize an empty HeadersTree and execute the actions on it while measuring the time taken.
    let store = if in_memory {
        Arc::new(InMemConsensusStore::new())
    } else {
        let tempdir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ChainStore<BlockHeader>> = Arc::new(
            RocksDBStore::create(RocksDbConfig::new(tempdir.path().to_path_buf())).unwrap(),
        );
        store
    };

    let mut headers_tree = HeadersTree::new(store.clone(), max_length);
    for header in tree.nodes() {
        store.store_header(&header).unwrap();
    }
    store.set_anchor_hash(&tree.value.hash()).unwrap();
    store.set_best_chain_hash(&tree.value.hash()).unwrap();

    let guard = if profile {
        ProfilerGuardBuilder::default().frequency(1000).build().ok()
    } else {
        None
    };

    let start = std::time::Instant::now();
    eprintln!("start executing the actions");
    let results = execute_actions_on_tree(store, &mut headers_tree, actions, false).unwrap();

    let elapsed = start.elapsed();
    let time_per_action = elapsed / (actions.len() as u32);

    if profile && let Some(report) = guard.and_then(|g| g.report().build().ok()) {
        let file = File::create("headers-tree-flamegraph.svg").unwrap();
        report
            .flamegraph_with_options(file, &mut Options::default())
            .unwrap();
    }

    eprintln!("tree size: {}", tree.size());
    eprintln!("tree leaves: {}", tree.leaves().len());
    eprintln!("number of peers: {}", peers_nb);
    eprintln!("number of actions: {}", actions.len());
    eprintln!(
        "number of rollbacks: {}",
        actions.iter().filter(|a| a.is_rollback()).count()
    );
    eprintln!("time after executing actions: {:?}", elapsed);
    eprintln!("time per action: {:?}", time_per_action);

    eprintln!(
        "headers best chain size after executing actions: {}",
        headers_tree.best_length()
    );
    eprintln!("number of results: {}", results.len());

    assert!(time_per_action.as_micros() < 1000);
}

/// On Windows, benchmarking is not supported because we hit a stack overflow error during the generation.
#[cfg(not(all(unix, feature = "profiling", feature = "test-utils")))]
fn main() {}
