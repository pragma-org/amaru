use std::env::{set_var, var};
use tracing::info;

#[tokio::main]
async fn main() {
    match var("RUST_LOG") {
        Ok(_) => {}
        Err(_) => {
            // set a default logging level of info if unset.
            set_var("RUST_LOG", "info");
        }
    }
    pretty_env_logger::init_timed();

    let tracing_filter = match var("RUST_LOG") {
        Ok(level) => match level.to_lowercase().as_str() {
            "error" => tracing::Level::ERROR,
            "warn" => tracing::Level::WARN,
            "info" => tracing::Level::INFO,
            "debug" => tracing::Level::DEBUG,
            "trace" => tracing::Level::TRACE,
            _ => tracing::Level::INFO,
        },
        Err(_) => tracing::Level::INFO,
    };

    tracing::subscriber::set_global_default(
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing_filter)
            .finish(),
    )
    .unwrap();

    info!("Hello, amaru-node!");
}
