#![forbid(unsafe_code)]

//! Zicade binary entrypoint. Parse the command line into a [`Command`], then
//! dispatch: console mode (the unchanged default) serves until Ctrl-C; service
//! mode hands control to the SCM dispatcher and serves until STOP/SHUTDOWN;
//! install/uninstall register or remove the Windows service. All server wiring
//! lives in the library ([`zicade`]); `main` owns process concerns (arg
//! parsing, the tokio runtime, the global subscriber, signals, exit code).
//!
//! This file stays `#![forbid(unsafe_code)]`: every SCM/FFI call is delegated to
//! [`zicade_win::service`].

use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use zicade::cli::{Command, DEFAULT_SERVICE_NAME, LaunchMode, launch_mode, parse_args};
use zicade::{LOG_BUFFER_CAP, init_tracing};
use zicade_config::Config;
use zicade_observe::channel_layer;

mod tray;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Command::Default => run_default(),
        Command::Run { config_path } => run_console(config_path),
        Command::ServiceRun => run_service(),
        Command::ServiceInstall { name } => install_service(name),
        Command::ServiceUninstall { name } => uninstall_service(name),
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
        LaunchMode::Tray => tray::run_tray_mode(),
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

/// Service mode: hand control to the SCM dispatcher. The service body builds
/// its own runtime and serves until the SCM STOP/SHUTDOWN triggers `stop`,
/// which the app awaits as its shutdown signal.
fn run_service() -> ExitCode {
    let result = zicade_win::service::run_dispatcher(DEFAULT_SERVICE_NAME, |stop| {
        let runtime = match tokio::runtime::Runtime::new() {
            Ok(runtime) => runtime,
            Err(err) => {
                eprintln!("zicade: failed to start the async runtime: {err}");
                return;
            }
        };
        let shutdown = async move {
            stop.wait().await;
            tracing::info!("service STOP/SHUTDOWN received; shutting down");
        };
        if let Err(err) = runtime.block_on(serve(None, shutdown)) {
            tracing::error!(error = %format!("{err:#}"), "service exited with an error");
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("zicade: service dispatcher error: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Install the Windows service pointing at the current exe with `service run`.
fn install_service(name: Option<String>) -> ExitCode {
    let name = name.unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned());
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("zicade: cannot resolve the current executable path: {err}");
            return ExitCode::FAILURE;
        }
    };
    let config = zicade_win::service::ServiceInstall {
        name: name.clone(),
        display_name: "Zicade Proxy".to_owned(),
        description: "Local HTTP/HTTPS forward proxy for domain-joined Windows.".to_owned(),
        exe_path: exe,
        args: vec!["service".to_owned(), "run".to_owned()],
    };
    match zicade_win::service::install(&config) {
        Ok(()) => {
            println!(
                "Installed service '{name}' (auto-start). Start it now with:\n  \
                 sc start {name}\nStop maps to graceful shutdown."
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "zicade: failed to install service '{name}': {err}\n\
                 (installing a service requires an elevated/Administrator prompt)"
            );
            ExitCode::FAILURE
        }
    }
}

/// Uninstall (delete) the Windows service.
fn uninstall_service(name: Option<String>) -> ExitCode {
    let name = name.unwrap_or_else(|| DEFAULT_SERVICE_NAME.to_owned());
    match zicade_win::service::uninstall(&name) {
        Ok(()) => {
            println!("Uninstalled service '{name}'.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!(
                "zicade: failed to uninstall service '{name}': {err}\n\
                 (removing a service requires an elevated/Administrator prompt)"
            );
            ExitCode::FAILURE
        }
    }
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
         zicade service run             Run under the Service Control Manager\n  \
         zicade service install [--name NAME]    Install the Windows service (admin)\n  \
         zicade service uninstall [--name NAME]  Uninstall the Windows service (admin)\n  \
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
