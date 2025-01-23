use amaru::consensus::Point;
use clap::Parser;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub struct Args {
    /// Address of the node to connect to for retrieving chain data.
    /// The node should be accessible via the node-2-node protocol, which
    /// means the remote node should be running as a validator and not
    /// as a client node.
    ///
    /// Addressis given in the usual `host:port` format, for example: "1.2.3.4:3000".
    #[arg(long, verbatim_doc_comment)]
    peer: String,

    /// Path of the on-disk storage.
    ///
    /// This is the directory where data will be stored. The directory and any intermediate
    /// paths will be created if they do not exist.
    #[arg(long, verbatim_doc_comment, default_value = super::DEFAULT_CHAIN_DATABASE_PATH)]
    chain_database_dir: PathBuf,

    /// Starting point of import.
    ///
    /// This is the "intersection" point which will be given to the peer as a starting point
    /// to import the chain database.
    #[arg(long, verbatim_doc_comment)]
    starting_point: String,
}

pub async fn run(_args: Args) -> miette::Result<()> {
    Ok(())
}
