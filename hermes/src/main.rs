#![feature(never_type)]
#![feature(slice_group_by)]
#![feature(btree_cursors)]

use {
    crate::store::Store,
    anyhow::Result,
    futures::future::join_all,
    std::sync::atomic::AtomicBool,
    structopt::StructOpt,
    tokio::spawn,
};

mod api;
mod config;
mod doc_examples;
mod macros;
mod network;
mod store;

// A static exit flag to indicate to running threads that we're shutting down. This is used to
// gracefully shutdown the application.
//
// NOTE: A more idiomatic approach would be to use a tokio::sync::broadcast channel, and to send a
// shutdown signal to all running tasks. However, this is a bit more complicated to implement and
// we don't rely on global state for anything else.
pub(crate) static SHOULD_EXIT: AtomicBool = AtomicBool::new(false);

/// Initialize the Application. This can be invoked either by real main, or by the Geyser plugin.
async fn init() -> Result<()> {
    log::info!("Initializing Hermes...");

    // Parse the command line arguments with StructOpt, will exit automatically on `--help` or
    // with invalid arguments.
    match config::Options::from_args() {
        config::Options::Run(opts) => {
            log::info!("Starting hermes service...");

            // The update channel is used to send store update notifications to the public API.
            let (update_tx, update_rx) = tokio::sync::mpsc::channel(1000);

            // Initialize a cache store with a 1000 element circular buffer.
            let store = Store::new(update_tx.clone(), 1000);

            // Listen for Ctrl+C so we can set the exit flag and wait for a graceful shutdown. We
            // also send off any notifications needed to close off any waiting tasks.
            spawn(async move {
                tokio::signal::ctrl_c().await.unwrap();
                log::info!("Shut down signal received, waiting for tasks...");
                SHOULD_EXIT.store(true, std::sync::atomic::Ordering::Release);
                let _ = update_tx.send(()).await;
            });

            // Spawn all worker tasks, and wait for all to complete (which will happen if a shutdown
            // signal has been observed).
            let tasks = join_all([
                Box::pin(spawn(network::p2p::spawn(opts.clone(), store.clone()))),
                Box::pin(spawn(network::pythnet::spawn(opts.clone(), store.clone()))),
                Box::pin(spawn(api::run(opts.clone(), store.clone(), update_rx))),
            ])
            .await;

            for task in tasks {
                task??;
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    // Launch the application. If it fails, print the full backtrace and exit. RUST_BACKTRACE
    // should be set to 1 for this otherwise it will only print the top-level error.
    if let Err(result) = init().await {
        eprintln!("{}", result.backtrace());
        result.chain().for_each(|cause| eprintln!("{cause}"));
        std::process::exit(1);
    }

    Ok(())
}
