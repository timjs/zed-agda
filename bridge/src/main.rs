//! `agda-bridge`: a language server that lets Zed drive Agda interactively
//! through Agda's own `--interaction-json` protocol, without the Agda
//! Language Server. See `docs/PLAN.md` in the repository.
//!
//! - `agda-bridge` (or `agda-bridge --stdio`): run the language server.
//! - `agda-bridge client …`: send a command to a running server, for debugging.

mod agda;
mod goals;
mod highlight;
mod input;
mod iotcm;
mod links;
mod location;
mod output;
mod protocol;
mod rename;
mod render;
mod server;
mod settings;
mod socket;
mod text;

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("--stdio") => server::run().await,
        Some("client") => std::process::exit(socket::run_client(&args[1..]).await),
        Some("--version") => println!("agda-bridge {}", env!("CARGO_PKG_VERSION")),
        Some(other) => {
            eprintln!(
                "unknown argument `{other}`; run without arguments for the language server, or `client` to send a command to a running server"
            );
            std::process::exit(2);
        }
    }
}
