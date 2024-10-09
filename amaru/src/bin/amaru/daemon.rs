use clap::Parser;

use super::Config;

#[derive(Debug, Parser)]
pub struct Args {}

pub fn run(config: Config, args: Args) -> miette::Result<()> {
    Ok(())
}
