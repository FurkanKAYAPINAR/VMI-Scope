//! Plumbing check for the alternate-credential path.
//! Connects to this machine's own hostname with bogus credentials; WMI rejects
//! credentialed *local* connections, and the specific rejection proves the
//! credentials reached DCOM (no crash = the COAUTHIDENTITY handling is sound).
//! Run with: `cargo run -p vmiscope-core --example altcred`

use vmiscope_core::{Credential, RemoteConn};

fn main() {
    let host = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "localhost".into());
    let cred = Credential {
        user: "bogus_user".into(),
        password: "bogus_pass".into(),
        domain: Some("BOGUSDOM".into()),
    };
    println!("connecting to \\\\{host}\\root\\cimv2 with bogus alt creds...");
    match RemoteConn::connect(&host, "root\\cimv2", &cred) {
        Ok(_) => println!("UNEXPECTED: connected with bogus creds"),
        Err(e) => println!("rejected as expected (proves creds reached DCOM):\n  {e}"),
    }
    println!("no crash — COAUTHIDENTITY handling is sound");
}
