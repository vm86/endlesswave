use clap::Parser;
use radio_core::daemon::{Options, migrate_files, run};

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let options = Options::parse();
    let level = if options.debug { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| level.into()),
        )
        .init();
    migrate_files();
    let stop = async {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(_) => std::future::pending().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            () = terminate => {}
        }
    };
    run(options, None, stop).await
}
