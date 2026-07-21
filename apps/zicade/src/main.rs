#![forbid(unsafe_code)]

//! Zicade binary entrypoint: resolve the config path, load (or create) the
//! config, install the global tracing subscriber, and serve the proxy + web
//! servers until Ctrl-C. All wiring lives in the library ([`zicade`]) so it is
//! testable; `main` only handles process concerns (config discovery, the global
//! subscriber, signal handling, and the process exit code).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use zicade::{LOG_BUFFER_CAP, init_tracing};
use zicade_config::Config;
use zicade_observe::channel_layer;

#[tokio::main]
async fn main() -> ExitCode {
    match real_main().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Config/subscriber errors can precede tracing setup, so print
            // directly. `{err:#}` renders the full anyhow context chain.
            eprintln!("zicade: fatal error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

async fn real_main() -> anyhow::Result<()> {
    let config_path = match std::env::args().nth(1) {
        Some(arg) => PathBuf::from(arg),
        None => default_config_path()?,
    };
    let config = load_or_create_config(&config_path)?;

    let (observe_layer, logs) = channel_layer(LOG_BUFFER_CAP);
    init_tracing(&config.logging, observe_layer);

    let shutdown = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "failed to listen for Ctrl-C; shutting down");
        } else {
            tracing::info!("Ctrl-C received; shutting down");
        }
    };

    zicade::run(config, config_path, logs, shutdown).await
}

/// Default config location: `%LOCALAPPDATA%\Zicade\config.json`.
fn default_config_path() -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .context("LOCALAPPDATA is not set; pass a config path as the first argument")?;
    Ok(Path::new(&base).join("Zicade").join("config.json"))
}

/// Load the config, or write out the defaults on first run.
fn load_or_create_config(path: &Path) -> anyhow::Result<Config> {
    if path.exists() {
        return zicade_config::load_file(path)
            .with_context(|| format!("failed to load config from {}", path.display()));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create config directory {}", dir.display()))?;
    }
    let config = Config::default();
    zicade_config::save_file(path, &config)
        .with_context(|| format!("failed to write default config to {}", path.display()))?;
    Ok(config)
}
