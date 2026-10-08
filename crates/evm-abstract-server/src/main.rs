//! Command-line ownership of the loopback web host.

use clap::Parser;
use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::ExitCode,
};

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
    let listener = TcpListener::bind(args.bind)?;
    println!("EVM workbench: http://{}", listener.local_addr()?);
    evm_abstract_server::http::serve(&listener, &args.assets)
}
