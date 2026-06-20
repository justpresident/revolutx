//! Credential resolution: build a client from the encrypted vault (prompting
//! for the master password) or, with `--insecure-env`, from environment
//! variables. Also implements `vault init`.

use std::path::PathBuf;
use std::sync::Arc;

use revolutx::{ClientConfig, Environment, Keystore, KeystoreOptions, RevolutXClient};
use zeroize::Zeroize;

use crate::args::{EnvArg, GlobalOpts, VaultCmd};

type Res<T> = Result<T, Box<dyn std::error::Error>>;

pub const fn environment(global: &GlobalOpts) -> Environment {
    match global.env {
        EnvArg::Production => Environment::Production,
        EnvArg::Dev => Environment::Dev,
    }
}

/// The vault path: `--vault`, else `$XDG_CONFIG_HOME/revolutx/vault`, else
/// `$HOME/.config/revolutx/vault`.
pub fn vault_path(global: &GlobalOpts) -> PathBuf {
    if let Some(path) = &global.vault {
        return path.clone();
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("revolutx").join("vault")
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
            "no vault at {} — run `revolutx vault init --key-file <pem>` first, or use --insecure-env",
            path.display()
        )
        .into());
    }

    let mut password = rpassword::prompt_password("Master password: ")?;
    let keystore = Keystore::open_with(&path, &password, &keystore_options(global));
    password.zeroize();
    let keystore = keystore?;

    Ok(RevolutXClient::builder()
        .environment(env)
        .signer(Arc::new(keystore))
        .build()?)
}

/// Runs a `vault` subcommand (synchronous — no network).
pub fn run_vault(global: &GlobalOpts, command: &VaultCmd) -> Res<()> {
    match command {
        VaultCmd::Init { key_file, api_key } => {
            let pem = std::fs::read_to_string(key_file)?;
            let path = vault_path(global);
            if path.exists() {
                return Err(format!("a vault already exists at {}", path.display()).into());
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }

            let mut api_key = match api_key {
                Some(key) => key.clone(),
                None => rpassword::prompt_password("API key: ")?,
            };
            let mut password = rpassword::prompt_password("New master password: ")?;
            let mut confirm = rpassword::prompt_password("Confirm master password: ")?;
            if password != confirm {
                password.zeroize();
                confirm.zeroize();
                api_key.zeroize();
                return Err("passwords do not match".into());
            }

            let result =
                Keystore::create_with(&path, &password, &api_key, &pem, &keystore_options(global));
            password.zeroize();
            confirm.zeroize();
            api_key.zeroize();
            result?;

            println!("Vault created at {}", path.display());
            Ok(())
        }
    }
}
