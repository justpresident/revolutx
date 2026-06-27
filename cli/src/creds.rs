//! Credential resolution: build a client from the encrypted vault (driving
//! rcypher's interactive multi-factor unlock) or, with `--insecure-env`, from
//! environment variables. Also implements `vault init`.
//!
//! The vault is rcypher's own `SecretStore` format, so it can also be inspected
//! and managed with the `rcypher` command-line tool — e.g. to enrol a FIDO2
//! security key, change the password, or rotate the API key.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rcypher::cli::{
    NoProgress, confirm_if_weak_password, get_password, prompt_password, prompt_until_unlocked,
};
use rcypher::{Argon2Params, LockedContainer, SecretStore};
use revolutx::{ClientConfig, Environment, Keystore, RevolutXClient};
use zeroize::Zeroizing;

use crate::args::{EnvArg, GlobalOpts, VaultCmd};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

/// The factor name given to the initial password when a vault is created.
const PRIMARY_FACTOR: &str = "primary";

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

    let keystore = unlock_vault(&path)?;
    Ok(RevolutXClient::builder()
        .environment(env)
        .signer(Arc::new(keystore))
        .build()?)
}

/// Loads the vault and drives rcypher's interactive unlock loop (prompting for
/// the password and — when built with the `fido2` feature — a security key, per
/// the vault's access policy), returning the unlocked [`Keystore`] signer.
fn unlock_vault(path: &Path) -> Res<Keystore> {
    let mut locked = LockedContainer::load(path)?;
    eprintln!("Unlock {}: {}", path.display(), locked.requirement());
    prompt_until_unlocked(&mut locked, &mut NoProgress)?;
    let unlocked = locked.unlock::<SecretStore>()?;
    Ok(Keystore::from_unlocked(unlocked))
}

/// Runs a `vault` subcommand (synchronous — no network).
pub fn run_vault(global: &GlobalOpts, command: &VaultCmd) -> Res<()> {
    let VaultCmd::Init { key_file } = command;
    init_vault(global, key_file.as_deref())
}

/// One-time vault setup: master password → key pair (generated, or imported with
/// `--key-file`) → API key → encrypted `SecretStore`. Every secret is `Zeroizing`,
/// so it is wiped on any exit, including the early returns below.
fn init_vault(global: &GlobalOpts, key_file: Option<&Path>) -> Res<()> {
    let path = vault_path(global);
    if path.exists() {
        return Err(format!("a vault already exists at {}", path.display()).into());
    }
    create_private_dir(path.parent())?;

    // 1. Choose the master password (shows the unrecoverable-password warning and
    //    confirms it), gated against weak choices, then create the store. The
    //    password is wiped as soon as the store's key is derived.
    let password = get_password(&path, true)?;
    if !confirm_if_weak_password(&password, &[PRIMARY_FACTOR, "revolutx"])? {
        return Err("vault creation cancelled (weak password not confirmed)".into());
    }
    let mut keystore = Keystore::create(PRIMARY_FACTOR, &password, &Argon2Params::default())?;
    drop(password);

    // 2. Put the private key into the store. Generate a fresh pair (default) or
    //    import an existing PEM.
    if let Some(key_file) = key_file {
        let pem = Zeroizing::new(std::fs::read_to_string(key_file)?);
        keystore.set(Keystore::PRIVATE_KEY_PEM, &pem)?;
    } else {
        let pair = revolutx::generate_key_pair()?;
        keystore.set(Keystore::PRIVATE_KEY_PEM, &pair.private_pem)?;
        print_onboarding(&pair.public_pem);
    }

    // 3. Put the API key into the store — created on the website using the public
    //    key above, then pasted here (hidden input, like a password).
    let api_key = prompt_password("Paste your API key")?;
    keystore.set(Keystore::API_KEY, &api_key)?;
    drop(api_key);

    // 4. Persist the encrypted vault to disk.
    keystore.save(&path)?;

    println!(
        "\nVault created at {}. Initialization complete.",
        path.display()
    );
    println!(
        "Tip: add a FIDO2 security key or change the password with the `rcypher` CLI — \
         the vault is rcypher's standard format."
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
