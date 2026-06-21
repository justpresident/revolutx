//! Credential resolution: build a client from the encrypted vault (prompting
//! for the master password) or, with `--insecure-env`, from environment
//! variables. Also implements `vault init`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use revolutx::{ClientConfig, Environment, Keystore, KeystoreOptions, RevolutXClient};
use zeroize::Zeroizing;

use crate::args::{EnvArg, GlobalOpts, VaultCmd};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

const fn environment(global: &GlobalOpts) -> Environment {
    match global.env {
        EnvArg::Production => Environment::Production,
        EnvArg::Dev => Environment::Dev,
    }
}

/// The vault path: `--vault`, else `~/.revolutx/vault`.
pub fn vault_path(global: &GlobalOpts) -> PathBuf {
    if let Some(path) = &global.vault {
        return path.clone();
    }
    std::env::var_os("HOME")
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
        .join(".revolutx")
        .join("vault")
}

fn keystore_options(global: &GlobalOpts) -> KeystoreOptions {
    KeystoreOptions {
        // Secure Argon2 always (must match what `vault init` used); only the
        // anti-debug toggle is exposed.
        trace_detection: !global.insecure_allow_debugging,
        ..Default::default()
    }
}

/// Builds a client. Public commands (`needs_auth = false`) get a credential-less
/// client; otherwise credentials come from the vault (default) or `REVOLUTX_*`
/// env vars (`--insecure-env`).
pub fn client(global: &GlobalOpts, needs_auth: bool) -> Res<RevolutXClient> {
    let env = environment(global);

    if !needs_auth {
        return Ok(RevolutXClient::builder().environment(env).build()?);
    }

    if global.insecure_env {
        let mut config = ClientConfig::from_env();
        config.environment = Some(env);
        return Ok(config.build()?);
    }

    let path = vault_path(global);
    if !path.exists() {
        return Err(format!(
            "no vault at {} — run `revolutx vault init` first, or use --insecure-env",
            path.display()
        )
        .into());
    }

    // Read the master password (wiped on drop) and unlock the vault.
    let password = Zeroizing::new(rpassword::prompt_password("Master password: ")?);
    let keystore = Keystore::open_with(&path, &password, &keystore_options(global))?;

    Ok(RevolutXClient::builder()
        .environment(env)
        .signer(Arc::new(keystore))
        .build()?)
}

/// Runs a `vault` subcommand (synchronous — no network).
pub fn run_vault(global: &GlobalOpts, command: &VaultCmd) -> Res<()> {
    let VaultCmd::Init { key_file } = command;
    init_vault(global, key_file.as_deref())
}

/// One-time vault setup: master password → key pair (generated, or imported with
/// `--key-file`) → API key → encrypted vault. Every secret is `Zeroizing`, so it
/// is wiped on any exit, including the early returns below.
fn init_vault(global: &GlobalOpts, key_file: Option<&Path>) -> Res<()> {
    let path = vault_path(global);
    if path.exists() {
        return Err(format!("a vault already exists at {}", path.display()).into());
    }
    create_private_dir(path.parent())?;
    let options = keystore_options(global);

    // 1. Set the master password (with confirmation), initialize the vault (which
    //    derives its key), and wipe the password immediately — it is not needed
    //    past this point.
    let password = Zeroizing::new(rpassword::prompt_password("New master password: ")?);
    let confirm = Zeroizing::new(rpassword::prompt_password("Confirm master password: ")?);
    if password.as_str() != confirm.as_str() {
        return Err("passwords do not match".into());
    }
    drop(confirm);
    let vault = Keystore::init(&path, &password, &options)?;
    drop(password);

    // 2. Key material: generate a fresh pair (default) or import an existing PEM.
    let pem: Zeroizing<String> = if let Some(key_file) = key_file {
        Zeroizing::new(std::fs::read_to_string(key_file)?)
    } else {
        let pair = revolutx::generate_key_pair()?;
        print_onboarding(&pair.public_pem);
        pair.private_pem
    };

    // 3. The API key — created on the website using the public key above, then
    //    pasted here (hidden input, like a password).
    let api_key = Zeroizing::new(rpassword::prompt_password("Paste your API key: ")?);

    // 4. Write the credentials into the now-initialized vault.
    vault.store(&api_key, &pem)?;

    println!(
        "\nVault created at {}. Initialization complete.",
        path.display()
    );
    Ok(())
}

/// Prints the onboarding instructions shown after generating a key pair.
fn print_onboarding(public_pem: &str) {
    println!();
    println!("Generated a new Ed25519 key pair — the private key is stored only in your vault.");
    println!("To finish, create your Revolut X API key:");
    println!("  1. Log in to https://exchange.revolut.com");
    println!("  2. In your profile, create a new API key and paste in this PUBLIC key:");
    println!();
    print!("{public_pem}");
    println!();
    println!("Then paste the API key it gives you below.");
}

/// Creates the vault's parent directory privately (`0700` on unix).
#[cfg(unix)]
fn create_private_dir(dir: Option<&Path>) -> Res<()> {
    use std::os::unix::fs::DirBuilderExt;
    if let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: Option<&Path>) -> Res<()> {
    if let Some(dir) = dir.filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}
