//! Command-line ownership of the loopback web host.

use clap::Parser;
use std::{ffi::OsString, net::SocketAddr, path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    version,
    about = "Host the egui EVM analysis workbench and its native backend"
)]
struct Args {
    /// Listening address; the default only accepts local connections.
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: SocketAddr,
    /// Directory containing the compiled index.html, JavaScript and WebAssembly.
    #[arg(long, default_value = "dist")]
    assets: PathBuf,
    /// RPC HTTP(S) URL or JSON providers file path.
    ///
    /// A URL creates one provider with ID "default" and name "Default RPC".
    /// For multiple providers, pass a JSON file path. Omit to disable RPC input.
    #[arg(long, value_name = "URL_OR_FILE")]
    rpc_config: Option<OsString>,
    /// CPU analysis workers; defaults to half the host's available threads.
    #[arg(long)]
    workers: Option<usize>,
    /// Waiting analyses in addition to active workers; defaults to twice workers.
    #[arg(long)]
    queue_capacity: Option<usize>,
    /// Terminal tasks retained for inspection before the oldest are evicted.
    #[arg(long, default_value_t = 16)]
    retained_jobs: usize,
    /// Per-job work shared by the minimum model and optional rich presentation.
    #[arg(long, default_value_t = evm_abstract_server::analyze::DEFAULT_POSTPROCESS_WORK)]
    postprocess_work: usize,
    /// Persistent decompiler cache directory; defaults to XDG_CACHE_HOME or HOME/.cache.
    #[arg(long)]
    decompile_cache_dir: Option<PathBuf>,
    /// Maximum cached bytecode/settings groups, independently of retained task IDs.
    #[arg(long, default_value_t = 128)]
    decompile_cache_entries: usize,
    /// Maximum committed source, settings and result bytes in the decompiler cache.
    #[arg(long,default_value_t=1024*1024*1024)]
    decompile_cache_bytes: u64,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> Result<(), evm_abstract_server::http::ServerError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(run_async(args))
}

async fn run_async(args: Args) -> Result<(), evm_abstract_server::http::ServerError> {
    let mut config = evm_abstract_server::jobs::Config::default();
    if let Some(workers) = args.workers {
        config.workers = workers;
    }
    config.queue_capacity = args
        .queue_capacity
        .unwrap_or_else(|| config.workers.saturating_mul(2));
    config.retained_jobs = args.retained_jobs;
    let providers = args
        .rpc_config
        .as_deref()
        .map(evm_abstract_server::rpc_providers::Registry::from_argument)
        .transpose()
        .map_err(evm_abstract_server::http::ServerError::RpcConfig)?
        .unwrap_or_default();
    let directory = args
        .decompile_cache_dir
        .map(Ok)
        .unwrap_or_else(evm_abstract_server::decompile::cache::Config::default_directory)
        .map_err(evm_abstract_server::http::ServerError::Cache)?;
    let cache = evm_abstract_server::decompile::cache::Cache::persistent(
        evm_abstract_server::decompile::cache::Config {
            directory,
            max_entries: args.decompile_cache_entries,
            max_bytes: args.decompile_cache_bytes,
        },
    )
    .map_err(evm_abstract_server::http::ServerError::Cache)?;
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    println!(
        "EVM workbench: http://{} ({} analysis workers)",
        listener.local_addr()?,
        config.workers
    );
    evm_abstract_server::http::serve_with_cache(
        listener,
        &args.assets,
        config,
        providers,
        args.postprocess_work,
        cache,
    )
    .await
}
