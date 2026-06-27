//! Generate an Ed25519 key pair to register with Revolut X.
//!
//! Prints the PUBLIC key (paste it into the web app to create your API key) and
//! writes the PRIVATE key to `private.pem` (owner-only, refusing to overwrite an
//! existing file). The private key is the credential — keep it secret; the
//! `revolutx` CLI can instead store it in an encrypted vault.
//!
//! ```sh
//! cargo run --example generate_keypair
//! ```

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pair = revolutx::generate_key_pair()?;

    println!("Public key — register this in the Revolut X web app:\n");
    print!("{}", pair.public_pem);

    let path = "private.pem";
    if std::path::Path::new(path).exists() {
        eprintln!("\n{path} already exists — not overwriting; private key not written.");
        return Ok(());
    }
    write_private_key(path, &pair.private_pem)?;
    println!("\nPrivate key written to {path} — keep it secret.");
    Ok(())
}

/// Writes the private key, creating the file fresh and owner-only (`0600`) on
/// unix so the secret is not world-readable.
fn write_private_key(path: &str, pem: &str) -> std::io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(pem.as_bytes())
}
