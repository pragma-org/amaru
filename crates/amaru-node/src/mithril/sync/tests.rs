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

use std::{fs::File, sync::Mutex, time::Duration};

use amaru_consensus::{
    effects::{ConsensusMode, ResourceBlockValidation, ResourceEraHistory},
    performance::{Performance, ResourcePerformance},
};
use amaru_kernel::{Epoch, EraHistory, Header, IsHeader, cardano::network_block::EncodedTestBlock, make_header};
use amaru_ledger::store::ReadStore;
use amaru_ouroboros::{
    BaseReadChainStore, MockBlockValidator, Nonces, StoreError, WriteChainStore,
    in_memory_chain_store::InMemoryChainStore, overriding_chain_store::OverridingChainStore,
};
use amaru_protocols::store_effects::ResourceHeaderStore;
use amaru_pure_stage::{StageGraph, tokio::TokioBuilder};
use amaru_stores::rocksdb::{RocksDB, consensus::RocksDBStore};
use tempfile::tempdir;
use test_case::test_case;

use super::{
    recovery::{lock_sync_file, resolve_resume_point, sync_lock_path},
    *,
};
use crate::tests::configuration::NodeTestConfig;

#[derive(Default)]
struct RecordingObserver(Mutex<Vec<MithrilProgress>>);

impl MithrilObserver for RecordingObserver {
    fn on_progress(&self, progress: MithrilProgress) {
        self.0.lock().unwrap().push(progress);
    }
}

fn interrupted_ingestion_store() -> (Arc<InMemoryChainStore>, Header, Header) {
    let from = make_header(1, 1, None);
    let target = make_header(2, 2, Some(from.hash()));
    let store = Arc::new(InMemoryChainStore::new());
    store.store_header(&from).unwrap();
    store.roll_forward_chain(&from.point()).unwrap();
    store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
    (store, from, target)
}

#[test]
fn replay_anchor_catches_up_to_the_stable_horizon() {
    let store = InMemoryChainStore::new();
    let mut parent = None;
    let mut points = Vec::new();
    for height in 1..=3 {
        let header = make_header(height, height, parent);
        parent = Some(header.hash());
        store.store_header(&header).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
        points.push(header.point());
    }
    store.set_anchor_point(&points[0]).unwrap();

    advance_replay_anchor(&store, points[2], 2).unwrap();
    assert_eq!(store.get_anchor_point(), points[0]);

    for height in 4..=8 {
        let header = make_header(height, height, parent);
        parent = Some(header.hash());
        store.store_header(&header).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
        points.push(header.point());
    }
    advance_replay_anchor(&store, points[7], 2).unwrap();
    assert_eq!(store.get_anchor_point(), points[5]);
    advance_replay_anchor(&store, points[7], 2).unwrap();
    assert_eq!(store.get_anchor_point(), points[5]);

    let next = make_header(9, 9, parent);
    store.store_header(&next).unwrap();
    store.roll_forward_chain(&next.point()).unwrap();
    advance_replay_anchor(&store, next.point(), 2).unwrap();
    assert_eq!(store.get_anchor_point(), points[6]);

    advance_replay_anchor(&store, next.point(), 4).unwrap();
    assert_eq!(store.get_anchor_point(), points[6]);
    assert!(advance_replay_anchor(&store, points[7], 2).is_err());
    assert_eq!(store.get_anchor_point(), points[6]);
}

#[tokio::test]
async fn standalone_reconciliation_repairs_persisted_stores_and_is_idempotent() {
    for chain_ahead in [false, true] {
        let from = make_header(1, 1, None);
        let target = make_header(2, 2, Some(from.hash()));
        let ledger_tip = if chain_ahead { &from } else { &target };
        let test_config = NodeTestConfig::default();
        test_config.chain_store.store_header(ledger_tip).unwrap();
        test_config.chain_store.set_anchor_point(&ledger_tip.point()).unwrap();
        let config = test_config.make_node_configuration().unwrap();
        let ledger_dir = config.ledger_config.ledger_store.dir;
        let directory = tempdir().unwrap();
        let chain_config = RocksDbConfig::new(directory.path().join("chain"));
        {
            let store = RocksDBStore::open_and_migrate(&chain_config).unwrap();
            store.store_validated_header(&from, &Nonces::for_tests()).unwrap();
            store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
            store.roll_forward_chain(&from.point()).unwrap();
            if chain_ahead {
                store.set_block_valid(&target.hash(), true).unwrap();
                store.roll_forward_chain(&target.point()).unwrap();
            }
        }

        for expected in [
            StoreRecoveryOutcome::Recovered {
                before: if chain_ahead { target.point() } else { from.point() },
                after: ledger_tip.point(),
            },
            StoreRecoveryOutcome::AlreadyConsistent { point: ledger_tip.point() },
        ] {
            assert_eq!(recover_store_pair(&ledger_dir, &chain_config.dir).await.unwrap(), expected);
            let store = RocksDBStore::open(&chain_config).unwrap();
            assert_eq!(store.get_best_chain_tip(), ledger_tip.point());
            assert_eq!(
                store.load_header_with_validity(&target.hash()).unwrap().1,
                if chain_ahead { None } else { Some(true) }
            );
            let ledger = RocksDB::new(&RocksDbConfig::new(ledger_dir.clone())).unwrap();
            assert_eq!(NetworkPoint::from(ledger.tip().unwrap()), NetworkPoint::from(ledger_tip.point()));
        }
        acquire_sync_locks([ledger_dir.as_path(), chain_config.dir.as_path()]).unwrap();
    }
}

#[tokio::test]
async fn standalone_reconciliation_respects_synchronization_locks() {
    let directory = tempdir().unwrap();
    let ledger_dir = directory.path().join("ledger");
    let chain_dir = directory.path().join("chain");
    let cache_dir = directory.path().join("cache");
    for path in [&ledger_dir, &chain_dir, &cache_dir] {
        fs::create_dir(path).unwrap();
    }

    for path in [&ledger_dir, &chain_dir] {
        let locks = acquire_sync_locks([path.as_path()]).unwrap();
        let expected = fs::canonicalize(path).unwrap();
        assert!(matches!(
            recover_store_pair(&ledger_dir, &chain_dir).await,
            Err(MithrilSyncError::Concurrent { path: locked }) if locked == expected
        ));
        drop(locks);
        acquire_sync_locks([cache_dir.as_path(), ledger_dir.as_path(), chain_dir.as_path()]).unwrap();
    }
}

#[test]
fn recovery_operational_failures_do_not_recommend_rebootstrap() {
    for chain_ahead in [false, true] {
        for failure in [
            StoreError::WriteError { error: "disk full".to_owned() },
            StoreError::ReadError { error: "I/O failure".to_owned() },
        ] {
            let from = make_header(1, 1, None);
            let target = make_header(2, 2, Some(from.hash()));
            let store = Arc::new(InMemoryChainStore::new());
            store.store_validated_header(&from, &Nonces::for_tests()).unwrap();
            store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
            store.roll_forward_chain(&from.point()).unwrap();
            if chain_ahead {
                store.roll_forward_chain(&target.point()).unwrap();
            }
            let before = store.get_best_chain_tip();
            let ledger_tip = if chain_ahead { from.point() } else { target.point() };
            let anchor_error = failure.clone();
            let validity_error = failure.clone();
            let failing = OverridingChainStore::builder(store.clone())
                .with_set_anchor_point(move |_, _| Err(anchor_error.clone()))
                .with_set_block_valid(move |_, _, _| Err(validity_error.clone()))
                .build();

            let error = recover_stores(&failing, ledger_tip).unwrap_err();
            assert!(matches!(error, MithrilSyncError::Store { source, .. }
                    if source.downcast_ref::<StoreError>() == Some(&failure)));
            assert_eq!(store.get_best_chain_tip(), before);
        }
    }
}

#[test]
fn unsupported_recovery_does_not_mutate_stores() {
    let parent = make_header(1, 1, None);
    let adopted = make_header(2, 2, Some(parent.hash()));
    let ledger = make_header(2, 3, Some(parent.hash()));
    let store = Arc::new(InMemoryChainStore::new());
    for header in [&parent, &adopted, &ledger] {
        store.store_validated_header(header, &Nonces::for_tests()).unwrap();
    }
    store.roll_forward_chain(&parent.point()).unwrap();
    store.roll_forward_chain(&adopted.point()).unwrap();
    let guarded = OverridingChainStore::builder(store.clone())
        .with_set_anchor_point(|_, _| panic!("unsupported recovery must not change the anchor"))
        .with_set_block_valid(|_, _, _| panic!("unsupported recovery must not change validity"))
        .build();

    assert!(matches!(recover_stores(&guarded, ledger.point()), Err(MithrilSyncError::RebootstrapRequired(_))));
    assert_eq!(store.get_best_chain_tip(), adopted.point());
}

#[tokio::test]
async fn worker_shutdown_failure_takes_precedence_over_ingestion_errors() {
    for ingestion_error in [
        MithrilSyncError::Cancelled,
        MithrilSyncError::InvalidCache { source: anyhow!("invalid block") },
        MithrilSyncError::RebootstrapRequired(Box::new(RebootstrapRequired {
            ledger_tip: Point::Origin,
            chain_tip: Point::Origin,
            reason: "unsupported interrupted state".to_owned(),
        })),
    ] {
        let test_config = NodeTestConfig::default();
        let header = make_header(1, 1, None);
        test_config.chain_store.store_header(&header).unwrap();
        test_config.chain_store.set_anchor_point(&header.point()).unwrap();
        let config = test_config.make_node_configuration().unwrap();
        let idle_references = Arc::strong_count(&test_config.chain_store);
        let (validator, running) = launch_stages(&config, &tokio::runtime::Handle::current(), |builder| {
            Ok(prepare_node(&config, builder, None, |_, _| Ok(()))?.0.block_validator)
        })
        .unwrap();
        let retained = validator.clone();
        let cancellation = MithrilCancellation::new();
        let mut reconciled = false;
        let result = complete_ingestion(
            async move {
                if matches!(ingestion_error, MithrilSyncError::Cancelled) {
                    cancellation.cancel();
                    cancellation.cancelled().await;
                }
                drop(validator);
                Err::<(Point, u64), _>(ingestion_error)
            },
            async {
                let report = running
                    .shutdown_with_timeout(Duration::ZERO)
                    .await
                    .map_err(|source| MithrilSyncError::WorkerShutdown { source: source.into() })?;
                assert!(report.is_clean());
                reconciled = true;
                Ok(Point::Origin)
            },
        )
        .await;

        drop(retained);
        tokio::time::timeout(Duration::from_secs(5), async {
            while Arc::strong_count(&test_config.chain_store) > idle_references {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!reconciled);
        assert!(matches!(result, Err(MithrilSyncError::WorkerShutdown { source })
                if matches!(source.downcast_ref::<crate::ShutdownError>(),
                    Some(crate::ShutdownError::LedgerTimeout { .. }))));
    }
}

#[tokio::test]
async fn zero_block_limit_skips_ingestion() {
    let directory = tempdir().unwrap();
    let synchronizer = MithrilSynchronizer::new(
        NetworkName::Preprod,
        directory.path().join("ledger"),
        directory.path().join("chain"),
        directory.path().join("snapshots"),
    )
    .ingest_limits(None, Some(0));
    let tip = make_header(1, 1, None);
    let store = Arc::new(InMemoryChainStore::new());
    store.store_header(&tip).unwrap();
    store.roll_forward_chain(&tip.point()).unwrap();
    let observer = RecordingObserver::default();

    let result = synchronizer
        .ingest(
            store.clone(),
            &directory.path().join("missing-immutable"),
            tip.point(),
            &MithrilCancellation::new(),
            &observer,
        )
        .await
        .unwrap();

    assert_eq!(result, (tip.point(), 0));
    assert_eq!(store.get_best_chain_tip(), tip.point());
    assert!(observer.0.lock().unwrap().is_empty());
    assert!(!synchronizer.ledger_dir.exists());
}

#[test_case(false; "valid_block")]
#[test_case(true; "rejected_block")]
#[tokio::test]
async fn mithril_blocks_use_the_shared_validation_and_adoption_stages(reject: bool) {
    let parent = make_header(1, 1, None);
    let block = EncodedTestBlock::from_seed(&make_header(2, 2, Some(parent.hash())), &EraHistory::default());
    let store = Arc::new(InMemoryChainStore::new());
    store.store_validated_header(&parent, &Nonces::for_tests()).unwrap();
    store.store_validated_header(&block.header, &Nonces::for_tests()).unwrap();
    store.roll_forward_chain(&parent.point()).unwrap();
    store.set_anchor_point(&parent.point()).unwrap();

    let mut builder = TokioBuilder::default();
    builder.resources().put::<ResourceHeaderStore>(store.clone());
    let validator = Arc::new(MockBlockValidator::new(parent.point()));
    if reject {
        validator.with_validate_fails(block.header.point());
    }
    builder.resources().put::<ResourceBlockValidation>(validator);
    builder.resources().put::<ResourcePerformance>(Arc::new(Performance::new()));
    builder.resources().put::<ResourceEraHistory>(EraHistory::default());
    builder.resources().put(ConsensusMode::Sync);
    let (source, mut events) = wire_mithril_stages(&mut builder, parent.point(), 2, None);
    let running = builder.run(tokio::runtime::Handle::current());
    source.send(MithrilInput::Block(block.raw)).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    match event {
        MithrilBlockEvent::Applied(point) => {
            assert!(!reject);
            assert_eq!(point, block.header.point());
        }
        MithrilBlockEvent::Failed(point, _) => {
            assert!(reject);
            assert_eq!(point, block.header.point());
        }
        MithrilBlockEvent::Finished => panic!("unexpected Mithril stage event"),
    }
    assert_eq!(store.load_header_with_validity(&block.header.hash()).unwrap().1, Some(!reject));
    assert_eq!(store.get_best_chain_tip(), if reject { parent.point() } else { block.header.point() });

    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[test]
fn forwards_download_and_verification_progress_in_order() {
    let observer = Arc::new(RecordingObserver::default());
    let forwarder = ForwardDownloadProgress(observer.clone());
    let stages = [
        (MithrilDownloadStage::FetchingSnapshot, MithrilStage::FetchingSnapshot),
        (MithrilDownloadStage::ValidatingCertificate, MithrilStage::ValidatingCertificate),
        (MithrilDownloadStage::Downloading { files: 6 }, MithrilStage::Downloading),
        (
            MithrilDownloadStage::VerifyingDatabase { from_chunk: 10, through_chunk: 11, files: 6 },
            MithrilStage::VerifyingDatabase { from_chunk: 10, through_chunk: 11, files: 6 },
        ),
        (MithrilDownloadStage::DatabaseVerified, MithrilStage::DatabaseVerified),
    ];
    let mut expected = Vec::new();
    for (download_stage, stage) in stages {
        forwarder.on_progress(MithrilDownloadProgress::StageChanged { stage: download_stage });
        expected.push(MithrilProgress::StageChanged { stage });
        if stage == MithrilStage::ValidatingCertificate {
            for _ in 0..2 {
                forwarder.on_progress(MithrilDownloadProgress::CertificateValidated);
                expected.push(MithrilProgress::CertificateValidated);
            }
        }
    }
    assert_eq!(*observer.0.lock().unwrap(), expected);
}

#[test_case(true; "dropped_future")]
#[test_case(false; "cancelled_token")]
#[tokio::test]
async fn cancellation_finishes_cleanup_and_releases_locks(abort: bool) {
    let directory = tempdir().unwrap();
    let locks = acquire_sync_locks([directory.path(); 3]).unwrap();
    let (store, from, target) = interrupted_ingestion_store();
    let target_point = target.point();
    let cleanup_store = store.clone();
    let cancellation = MithrilCancellation::new();
    let task_cancellation = cancellation.clone();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (stopping_tx, stopping_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (finished_tx, finished_rx) = tokio::sync::oneshot::channel();
    let caller = tokio::spawn(run_synchronization(task_cancellation, move |cancellation| async move {
        let (validator, worker_rx) = tokio::sync::oneshot::channel::<()>();
        let worker = tokio::task::spawn_blocking(move || {
            assert!(worker_rx.blocking_recv().is_err());
            target_point
        });
        let ingestion = async move {
            started_tx.send(()).unwrap();
            cancellation.cancelled().await;
            drop(validator);
            Err::<(Point, u64), _>(MithrilSyncError::Cancelled)
        };
        let cleanup = async move {
            stopping_tx.send(()).unwrap();
            release_rx.await.unwrap();
            let ledger_tip = worker.await.unwrap();
            recover_stores(cleanup_store.as_ref(), ledger_tip).map(StoreRecoveryOutcome::point)
        };
        let result = complete_ingestion(ingestion, cleanup).await;
        drop(locks);
        finished_tx.send(()).unwrap();
        result
    }));
    started_rx.await.unwrap();
    if abort {
        caller.abort();
    } else {
        cancellation.cancel();
    }
    tokio::time::timeout(Duration::from_secs(5), stopping_rx).await.unwrap().unwrap();
    assert_eq!(store.get_best_chain_tip(), from.point());
    assert!(matches!(acquire_sync_locks([directory.path(); 3]), Err(MithrilSyncError::Concurrent { .. })));
    if !abort {
        assert!(!caller.is_finished());
    }
    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), finished_rx).await.unwrap().unwrap();
    assert_eq!(store.get_best_chain_tip(), target.point());
    acquire_sync_locks([directory.path(); 3]).unwrap();
    if abort {
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(!cancellation.is_cancelled());
    } else {
        assert!(matches!(caller.await.unwrap(), Err(MithrilSyncError::Cancelled)));
    }
}

struct PanickingObserver;

impl MithrilObserver for PanickingObserver {
    fn on_progress(&self, _: MithrilProgress) {
        panic!("observer failed");
    }
}

#[tokio::test]
async fn observer_panics_join_the_worker_and_reconcile_stores() {
    for progress in [
        MithrilProgress::StageChanged { stage: MithrilStage::Ingesting },
        MithrilProgress::BlocksIngested { blocks: 1, point: Point::Origin },
    ] {
        let (store, _, target) = interrupted_ingestion_store();
        let target_point = target.point();
        let cleanup_store = store.clone();
        let result = run_synchronization(MithrilCancellation::new(), move |_| async move {
            let (validator, worker_rx) = tokio::sync::oneshot::channel::<()>();
            let worker = tokio::task::spawn_blocking(move || {
                assert!(worker_rx.blocking_recv().is_err());
                target_point
            });
            let ingestion = async move {
                PanickingObserver.on_progress(progress);
                drop(validator);
                Ok((Point::Origin, 0))
            };
            complete_ingestion(ingestion, async move {
                let ledger_tip = worker.await.unwrap();
                recover_stores(cleanup_store.as_ref(), ledger_tip).map(StoreRecoveryOutcome::point)
            })
            .await
        });
        let result = tokio::time::timeout(Duration::from_secs(5), result).await.unwrap();
        assert!(matches!(result, Err(MithrilSyncError::TaskFailed { source }) if source.is_panic()));
        assert_eq!(store.get_best_chain_tip(), target.point());
    }
}

#[tokio::test]
async fn cancelled_synchronization_never_reports_completion() {
    let directory = tempdir().unwrap();
    let cancellation = MithrilCancellation::new();
    cancellation.cancel();
    let observer = Arc::new(RecordingObserver::default());
    let synchronizer = MithrilSynchronizer::new(
        NetworkName::Preprod,
        directory.path().join("ledger"),
        directory.path().join("chain"),
        directory.path().join("snapshots"),
    );

    assert!(matches!(synchronizer.synchronize(cancellation, observer.clone()).await, Err(MithrilSyncError::Cancelled)));
    assert!(!observer.0.lock().unwrap().iter().any(|event| matches!(event, MithrilProgress::Completed { .. })));
}

#[tokio::test]
async fn recovery_precedes_an_unavailable_download() {
    let (store, _, target) = interrupted_ingestion_store();
    let observer = RecordingObserver::default();
    observer.on_progress(MithrilProgress::StageChanged { stage: MithrilStage::ResolvingResumePoint });

    let result = recover_then_download(
        store.as_ref(),
        target.point(),
        Some(NetworkPoint::from(target.point())),
        &MithrilCancellation::new(),
        &observer,
        async {
            assert_eq!(store.get_best_chain_tip(), target.point());
            Err(MithrilDownloadError::Unavailable { source: anyhow!("offline") })
        },
    )
    .await;

    assert!(matches!(result, Err(MithrilSyncError::SnapshotUnavailable { .. })));
    assert_eq!(store.get_best_chain_tip(), target.point());
    assert_eq!(
        observer.0.lock().unwrap().as_slice(),
        [MithrilStage::ResolvingResumePoint, MithrilStage::RecoveringStores, MithrilStage::Downloading]
            .map(|stage| MithrilProgress::StageChanged { stage })
    );
}

#[test]
fn synchronization_lock_covers_the_store_pair_across_cache_directories() {
    let directory = tempdir().unwrap();
    let ledger_dir = directory.path().join("ledger");
    let chain_dir = directory.path().join("chain");
    let first_cache = directory.path().join("cache-a");
    let second_cache = directory.path().join("cache-b");
    for path in [&ledger_dir, &chain_dir, &first_cache, &second_cache] {
        fs::create_dir(path).unwrap();
    }

    let locks = acquire_sync_locks([&first_cache, &ledger_dir, &chain_dir]).unwrap();
    assert!(directory.path().join(".ledger.mithril-sync.lock").exists());
    assert!(directory.path().join(".chain.mithril-sync.lock").exists());
    assert!(matches!(
        acquire_sync_locks([&second_cache, &ledger_dir, &chain_dir]),
        Err(MithrilSyncError::Concurrent { .. })
    ));
    drop(locks);
    for path in [&first_cache, &ledger_dir, &chain_dir] {
        assert!(!sync_lock_path(path).unwrap().exists());
    }
    let locks = acquire_sync_locks([&second_cache, &ledger_dir, &chain_dir]).unwrap();
    drop(locks);
    for path in [&second_cache, &ledger_dir, &chain_dir] {
        assert!(!sync_lock_path(path).unwrap().exists());
    }
}

#[cfg(unix)]
#[test]
fn replaced_lock_file_is_not_accepted() {
    let directory = tempdir().unwrap();
    let path = sync_lock_path(directory.path()).unwrap();
    let stale = File::create(&path).unwrap();
    fs::remove_file(&path).unwrap();
    File::create(&path).unwrap();

    assert!(matches!(lock_sync_file(&stale, &path, directory.path()), Err(MithrilSyncError::Concurrent { .. })));
}

#[derive(Clone, Copy)]
enum InterruptedAfter {
    BeforeWrites,
    HeaderAndNonces,
    LedgerCommit,
    Validity,
    ChainAdoption,
}

#[test_case(InterruptedAfter::BeforeWrites; "before_writes")]
#[test_case(InterruptedAfter::HeaderAndNonces; "header_and_nonces")]
#[test_case(InterruptedAfter::LedgerCommit; "ledger_commit")]
#[test_case(InterruptedAfter::Validity; "validity")]
#[test_case(InterruptedAfter::ChainAdoption; "chain_adoption")]
fn recovery_handles_each_cross_store_write_boundary(boundary: InterruptedAfter) {
    let from = make_header(1, 1, None);
    let target = make_header(2, 2, Some(from.hash()));
    let store = InMemoryChainStore::new();
    store.store_header(&from).unwrap();
    store.roll_forward_chain(&from.point()).unwrap();

    if !matches!(boundary, InterruptedAfter::BeforeWrites) {
        store.store_validated_header(&target, &Nonces::for_tests()).unwrap();
    }
    let ledger_tip = match boundary {
        InterruptedAfter::BeforeWrites | InterruptedAfter::HeaderAndNonces => from.point(),
        InterruptedAfter::LedgerCommit | InterruptedAfter::Validity | InterruptedAfter::ChainAdoption => target.point(),
    };
    if matches!(boundary, InterruptedAfter::Validity | InterruptedAfter::ChainAdoption) {
        store.set_block_valid(&target.hash(), true).unwrap();
    }
    if matches!(boundary, InterruptedAfter::ChainAdoption) {
        store.roll_forward_chain(&target.point()).unwrap();
    }

    recover_stores(&store, ledger_tip).unwrap();

    assert_eq!(store.get_best_chain_tip(), ledger_tip);
}

#[test]
fn recovery_requires_rebootstrap_when_the_ledger_target_is_missing() {
    let from: Header = make_header(1, 1, None);
    let target = make_header(2, 2, Some(from.hash()));
    let store = InMemoryChainStore::new();
    store.store_header(&from).unwrap();
    store.roll_forward_chain(&from.point()).unwrap();

    assert!(matches!(recover_stores(&store, target.point()), Err(MithrilSyncError::RebootstrapRequired(_))));
}

#[test]
fn resolves_the_bootstrap_ledger_tip_height_from_the_chain_store() {
    let tip = make_header(42, 123, None);
    let store = InMemoryChainStore::new();
    store.store_header(&tip).unwrap();

    let stored_tip = NetworkPoint::from(tip.point());
    let resolved = resolve_resume_point(&store, stored_tip).unwrap();

    assert_eq!(resolved, tip.point());
    assert_eq!(resolved.block_height(), 42.into());
}

#[test]
fn recovery_initializes_a_bootstrapped_store_without_a_best_chain() {
    let parent = make_header(41, 122, None);
    let tip = make_header(42, 123, Some(parent.hash()));
    let store = InMemoryChainStore::new();
    store.store_validated_header(&parent, &Nonces::for_tests()).unwrap();
    store.store_validated_header(&tip, &Nonces::for_tests()).unwrap();
    assert_eq!(store.get_best_chain_tip(), Point::Origin);

    recover_stores(&store, tip.point()).unwrap();

    assert_eq!(store.get_anchor_point(), tip.point());
    assert_eq!(store.get_best_chain_tip(), tip.point());
    assert!(store.is_on_best_chain(NetworkPoint::from(tip.point())));
}

#[test]
fn recovery_rewinds_the_adopted_chain_to_the_durable_ledger_tip() {
    let durable = make_header(1, 1, None);
    let volatile_1 = make_header(2, 2, Some(durable.hash()));
    let volatile_2 = make_header(3, 3, Some(volatile_1.hash()));
    let store = InMemoryChainStore::new();
    for header in [&durable, &volatile_1, &volatile_2] {
        store.store_validated_header(header, &Nonces::for_tests()).unwrap();
        store.set_block_valid(&header.hash(), true).unwrap();
        store.roll_forward_chain(&header.point()).unwrap();
    }

    recover_stores(&store, durable.point()).unwrap();

    assert_eq!(store.get_anchor_point(), durable.point());
    assert_eq!(store.get_best_chain_tip(), durable.point());
    assert_eq!(store.load_header_with_validity(&volatile_1.hash()).unwrap().1, None);
    assert_eq!(store.load_header_with_validity(&volatile_2.hash()).unwrap().1, None);
}

#[tokio::test]
async fn replay_retries_a_pending_block_on_stake_distribution_update() {
    use crate::mithril::{MithrilBlockSource, mithril_block_source};
    let target = Epoch::from(1120);
    let parent = make_header(1, 1, None);
    let block = EncodedTestBlock::from_seed(&make_header(2, 2, Some(parent.hash())), &EraHistory::default());
    let store = Arc::new(InMemoryChainStore::new());
    store.store_validated_header(&block.header, &Nonces::for_tests()).unwrap();
    let mut builder = TokioBuilder::default();
    builder.resources().put::<ResourceHeaderStore>(store.clone());
    let (validate_block, mut validations) = builder.output("validate", 1);
    let (events, _) = builder.output("events", 1);
    let stage = builder.stage("source", mithril_block_source);
    let stage = builder.wire_up(
        stage,
        MithrilBlockSource {
            validate_block,
            events,
            tip: parent.point(),
            until_slot: None,
            pending: Some((target, block.raw)),
        },
    );
    let sender = builder.input(stage);
    let running = builder.run(tokio::runtime::Handle::current());
    sender.send(MithrilInput::StakeDistUpdated(target)).await.unwrap();
    let message = tokio::time::timeout(Duration::from_secs(5), validations.next()).await.unwrap().unwrap();
    assert_eq!(
        message,
        amaru_consensus::stages::validate_block::ValidateBlockMsg::new(
            block.header.point(),
            parent.point(),
            block.header.point().block_height(),
        )
    );
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[test_case("parent"; "wrong_parent")]
#[test_case("height"; "skipped_height")]
#[test_case("cbor"; "malformed_block")]
#[test_case("limit"; "slot_limit")]
#[test_case("origin"; "origin_replay")]
#[tokio::test]
async fn replay_checks_input_before_dispatching_validation(case: &str) {
    let parent = make_header(1, 1, None);
    let tip = if case == "origin" { Point::Origin } else { parent.point() };
    let header = if case == "origin" {
        parent
    } else {
        make_header(
            if case == "height" { 3 } else { 2 },
            2,
            Some(if case == "parent" { Point::Origin.hash() } else { parent.hash() }),
        )
    };
    let block = EncodedTestBlock::from_seed(&header, &EraHistory::default());
    let raw = if case == "cbor" { RawBlock::from(vec![0xff].into_boxed_slice()) } else { block.raw };
    let mut builder = TokioBuilder::default();
    let (source, mut events) = wire_mithril_stages(&mut builder, tip, 2, (case == "limit").then_some(Slot::from(1)));
    let running = builder.run(tokio::runtime::Handle::current());
    source.send(MithrilInput::Block(raw)).await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(5), events.next()).await.unwrap().unwrap();
    if case == "limit" {
        assert_eq!(event, MithrilBlockEvent::Finished);
    } else if case == "origin" {
        assert_eq!(event, MithrilBlockEvent::Failed(header.point(), "replay from origin is not supported".into()));
    } else {
        assert!(matches!(event, MithrilBlockEvent::Failed(..)), "{event:?}");
    }
    running.request_abort();
    assert!(running.join().await.unwrap().unexpected_exits.is_empty());
}

#[tokio::test]
async fn failed_ingestion_releases_existing_rocksdb_stores() {
    let test_config = NodeTestConfig::default();
    let config = test_config.make_node_configuration().unwrap();
    let directory = tempdir().unwrap();
    let chain_config = RocksDbConfig::new(directory.path().join("chain"));
    let chain_store = Arc::new(RocksDBStore::open_and_migrate(&chain_config).unwrap());
    let synchronizer = MithrilSynchronizer::new(
        config.network(),
        config.ledger_config.ledger_store.dir.clone(),
        chain_config.dir.clone(),
        directory.path().join("snapshots"),
    );
    let result = synchronizer
        .ingest(
            chain_store.clone(),
            &directory.path().join("missing-immutable"),
            Point::Origin,
            &MithrilCancellation::new(),
            &RecordingObserver::default(),
        )
        .await;
    assert!(matches!(result, Err(MithrilSyncError::InvalidCache { .. })), "{result:?}");
    assert_eq!(Arc::strong_count(&chain_store), 1);
    drop(chain_store);
    drop(RocksDBStore::open(&chain_config).unwrap());
    drop(RocksDB::new(&config.ledger_config.ledger_store).unwrap());
}
