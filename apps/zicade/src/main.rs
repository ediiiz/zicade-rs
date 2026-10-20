#![forbid(unsafe_code)]

//! Zicade binary entrypoint. Parse the command line into a [`Command`], then
//! dispatch: console mode serves until Ctrl-C; a double-click runs in the
//! tray. All server wiring
//! lives in the library ([`zicade`]); `main` owns process concerns (arg
//! parsing, the tokio runtime, the global subscriber, signals, exit code).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use zicade::cli::{Command, LaunchMode, launch_mode, parse_args};
use zicade::{LOG_BUFFER_CAP, init_tracing};
use zicade_config::Config;
use zicade_observe::channel_layer;

mod tray;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Command::Default => run_default(),
        Command::Run { config_path } => run_console(config_path),
        Command::Background => tray::run_tray_mode(false),
        Command::Help => {
            print_usage();
            ExitCode::SUCCESS
        }
        Command::Unknown(token) => {
            eprintln!("zicade: unrecognized command: {token}\n");
            print_usage();
            ExitCode::FAILURE
        }
    }
}

/// The no-subcommand default (bare `zicade`). Branch on the double-click
/// heuristic: a freshly-created console we alone own (`GetConsoleProcessList`
/// == 1) means the exe was double-clicked, so run in the tray; otherwise this
/// was launched from a terminal, so serve in the console as before.
fn run_default() -> ExitCode {
    let count = zicade_win::tray::console_process_count();
    match launch_mode(count, &Command::Default) {
        LaunchMode::Tray => tray::run_tray_mode(true),
        LaunchMode::Background => tray::run_tray_mode(false),
        LaunchMode::Console => run_console(None),
    }
}

/// Console mode (default): serve until Ctrl-C.
fn run_console(config_path: Option<PathBuf>) -> ExitCode {
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("zicade: failed to start the async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let shutdown = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(%err, "failed to listen for Ctrl-C; shutting down");
        } else {
            tracing::info!("Ctrl-C received; shutting down");
        }
    };
    report(runtime.block_on(serve(config_path, shutdown)))
}

/// Load config, install the subscriber, and serve until `shutdown` resolves.
async fn serve(
    config_path: Option<PathBuf>,
    shutdown: impl Future<Output = ()> + Send,
) -> anyhow::Result<()> {
    let config_path = match config_path {
        Some(path) => path,
        None => default_config_path()?,
    };
    let config = load_or_create_config(&config_path)?;

    let (observe_layer, logs) = channel_layer(LOG_BUFFER_CAP);
    let log_reload = init_tracing(&config.logging, observe_layer);

    zicade::run(config, config_path, logs, log_reload, shutdown).await
}

/// Turn a serve result into a process exit code, printing any error.
fn report(result: anyhow::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Config/subscriber errors can precede tracing setup, so print
            // directly. `{err:#}` renders the full anyhow context chain.
            eprintln!("zicade: fatal error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Usage text for `--help` and unknown commands.
fn print_usage() {
    println!(
        "Zicade - local HTTP/HTTPS forward proxy\n\n\
         USAGE:\n  \
         zicade [CONFIG_PATH]            Run in the console (default); Ctrl-C to stop\n  \
         zicade run [CONFIG_PATH]        Same as above, explicit\n  \
         zicade --background            Run in the tray without opening the window\n  \
         zicade --help                  Show this help\n\n\
         CONFIG_PATH defaults to %LOCALAPPDATA%\\Zicade\\config.json."
    );
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
