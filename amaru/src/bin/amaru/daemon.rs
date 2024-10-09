use clap::Parser;

use super::Config;

#[derive(Debug, Parser)]
pub struct Args {}

pub async fn run(config: Config, args: Args) -> miette::Result<()> {
    crate::common::setup_tracing()?;

    let sync = amaru::sync::bootstrap(config.sync)?;

    let exit = crate::common::hook_exit_token();
    crate::common::run_pipeline(gasket::daemon::Daemon::new(sync), exit.clone()).await;

    Ok(())
}
