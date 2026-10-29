//! Read-only authenticated smoke test; never prints credentials or clipboard data.
use omaclipsync_core::{keyring, Server};
use std::env;
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: server_check HTTPS_URL USER");
        std::process::exit(2);
    }
    let password = match keyring::lookup(&args[2]) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("keyring lookup failed");
            std::process::exit(1);
        }
    };
    let server = match Server::new(&args[1], args[2].clone(), password) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("invalid URL or TLS configuration");
            std::process::exit(1);
        }
    };
    if server.check().is_err() {
        eprintln!("server connectivity check failed");
        std::process::exit(1);
    }
    match server.current() {
        Ok(p) if !p.hash.is_empty() && !p.kind.is_empty() => {
            println!("authenticated connectivity and metadata: OK")
        }
        _ => {
            eprintln!("metadata check failed");
            std::process::exit(1);
        }
    }
}
