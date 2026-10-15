//! Read-only EWS endpoint diagnostic. Configure EWS_URL, EWS_USERNAME and EWS_PASSWORD in the
//! environment. EWS_ALLOW_PLAINTEXT=1 is required to send credentials to an explicit HTTP endpoint.
//! No credential values are printed.

use std::env;

use mailcraft_net::{ConnectOptions, ews, http::HttpClient};

fn main() {
    if let Err(error) = run() {
        eprintln!("EWS probe failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> mailcraft_net::Result<()> {
    let url = env::var("EWS_URL").map_err(|_| mailcraft_net::NetError::Invalid("set EWS_URL".into()))?;
    let username =
        env::var("EWS_USERNAME").map_err(|_| mailcraft_net::NetError::Invalid("set EWS_USERNAME".into()))?;
    let password =
        env::var("EWS_PASSWORD").map_err(|_| mailcraft_net::NetError::Invalid("set EWS_PASSWORD".into()))?;
    let allow_plaintext_auth = env::var("EWS_ALLOW_PLAINTEXT").is_ok_and(|v| v == "1");
    let http = HttpClient::new(ConnectOptions { allow_plaintext_auth, ..ConnectOptions::default() });
    let folders = ews::probe(&url, &username, &password, &http)?;
    for folder in folders {
        match (folder.id, folder.error) {
            (Some(_), None) => println!("{} ({}): available", folder.kind, folder.distinguished_id),
            (_, Some(error)) => println!("{} ({}): {error}", folder.kind, folder.distinguished_id),
            _ => println!("{} ({}): unavailable", folder.kind, folder.distinguished_id),
        }
    }
    Ok(())
}
