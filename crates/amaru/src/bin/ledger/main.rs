use clap::Parser;
use std::{fs::read_dir, path::{PathBuf}, sync::{Arc, RwLock}};

use amaru::{observability::{self, OpenTelemetryConfig}, panic::panic_handler};
use amaru::stages::ledger::ValidateBlockStage;
use amaru_kernel::{default_ledger_dir, network::NetworkName, protocol_parameters::GlobalParameters, EraHistory, Point};
use amaru_stores::rocksdb::{RocksDB, RocksDBHistoricalStores};

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    /// The target network to choose from.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = NetworkName::Preprod,
    )]
    network: NetworkName,

    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    #[clap(long, action, env("AMARU_WITH_OPEN_TELEMETRY"))]
    with_open_telemetry: bool,

    #[clap(long, action, env("AMARU_WITH_JSON_TRACES"))]
    with_json_traces: bool,

    #[arg(long, value_name = "STRING", env("AMARU_SERVICE_NAME"), default_value_t = DEFAULT_SERVICE_NAME.to_string())]
    service_name: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_SPAN_URL"), default_value_t = DEFAULT_OTLP_SPAN_URL.to_string())]
    otlp_span_url: String,

    #[arg(long, value_name = "URL", env("AMARU_OTLP_METRIC_URL"), default_value_t = DEFAULT_OTLP_METRIC_URL.to_string())]
    otlp_metric_url: String,
}

const DEFAULT_SERVICE_NAME: &str = "amaru-ledger";

const DEFAULT_OTLP_SPAN_URL: &str = "http://localhost:4317";

const DEFAULT_OTLP_METRIC_URL: &str = "http://localhost:4318/v1/metrics";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    panic_handler();

    let args = Cli::parse();

    let mut subscriber = observability::TracingSubscriber::new();

    let observability::OpenTelemetryHandle { metrics: _, teardown } = if args.with_open_telemetry {
        observability::setup_open_telemetry(
            &OpenTelemetryConfig {
                service_name: args.service_name,
                span_url: args.otlp_span_url,
                metric_url: args.otlp_metric_url,
            },
            &mut subscriber,
        )
    } else {
        observability::OpenTelemetryHandle::default()
    };

    if args.with_json_traces {
        observability::setup_json_traces(&mut subscriber);
    }

    subscriber.init();


    let network = args.network;
    let ledger_dir = args
        .ledger_dir
        .unwrap_or_else(|| default_ledger_dir(network).into());

    let era_history: &EraHistory = network.into();
    let global_parameters: &GlobalParameters = network.into();
    let store = RocksDBHistoricalStores::new(&ledger_dir);
    let is_catching_up = Arc::new(RwLock::new(true));
    let (mut ledger, tip) = ValidateBlockStage::new(
        RocksDB::new(&ledger_dir).unwrap(),
        store,
        network,
        era_history.clone(),
        global_parameters.clone(),
        is_catching_up,
    )?;
 
    // Read all file names in data/blocks. File name format is POINT.HASH
    let mut points: Vec<_> = read_dir(format!("data/{}/blocks", network))?.into_iter().filter_map(|file| {
        let file = file.unwrap();
        let file_name = file.file_name();
        let (point, hash) = file_name.to_str().unwrap_or_default().split_once(".").unwrap_or_default();
        Some(Point::Specific(point.parse().unwrap_or_default(), hex::decode(hash).unwrap()))
    }).collect();

    // Sort them by slot number
    points.sort_by(|point1, point2| {
        point1.slot_or_default().cmp(&point2.slot_or_default())
    });

    // Then filter them to only keep those with a slot number greater than the tip's slot number
    let subset: &[Point] = match points.iter().position(|x| x.slot_or_default() > tip.slot_or_default()) {
        Some(pos) => &points[pos..],
        None => &[],
    };

    println!("Processing {} blocks from slot {}", subset.len(), tip.slot_or_default());

    for point in subset {
        if u64::from(point.slot_or_default()) % 100000 == 0 {
            println!("Processing block at slot {}", point.slot_or_default());
        }
        let file_path = format!("data/blocks/{}.{}", point.slot_or_default(), hex::encode(point.hash()));
        let raw_block = std::fs::read(&file_path)?;
        if let Err(err) = ledger.roll_forward(point.clone(), raw_block)? {
            panic!("Error processing block at point {:?}: {:?}", point, err);
        };
    }

    if let Err(report) = teardown() {
        eprintln!("Failed to teardown tracing: {report}");
    }

    Ok(())
}
