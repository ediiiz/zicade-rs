//! Command-line argument parsing for the Zicade binary.
//!
//! Parsing is a **pure** function ([`parse_args`]) over an argument slice
//! (the process args with `argv[0]` already stripped), returning a [`Command`].
//! Keeping it free of any side effects — no SCM calls, no runtime, no I/O —
//! makes the whole dispatch table unit-testable without Administrator or a real
//! Service Control Manager.

use std::path::PathBuf;

/// The default Windows service name used when `--name` is omitted.
pub const DEFAULT_SERVICE_NAME: &str = "Zicade";

/// A parsed top-level command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Console mode (the unchanged default): serve until Ctrl-C. An optional
    /// explicit config path preserves the historical "first arg is the config
    /// path" behavior.
    Run { config_path: Option<PathBuf> },
    /// Run under the Service Control Manager (`service run`): serve until the
    /// SCM sends STOP/SHUTDOWN.
    ServiceRun,
    /// Install the Windows service (`service install [--name NAME]`).
    ServiceInstall { name: Option<String> },
    /// Uninstall the Windows service (`service uninstall [--name NAME]`).
    ServiceUninstall { name: Option<String> },
    /// Print usage text (`--help`/`-h`/`help`).
    Help,
    /// An unrecognized command; carries the offending token(s) for the message.
    Unknown(String),
}

/// Parse `args` (process args with `argv[0]` stripped) into a [`Command`].
pub fn parse_args(args: &[String]) -> Command {
    let Some(first) = args.first() else {
        return Command::Run { config_path: None };
    };
    match first.as_str() {
        "--help" | "-h" | "help" => Command::Help,
        "run" => Command::Run {
            config_path: args.get(1).map(PathBuf::from),
        },
        "service" => parse_service(&args[1..]),
        // A leading flag we do not recognize is an error...
        other if other.starts_with('-') => Command::Unknown(other.to_owned()),
        // ...but any other bare token is treated as an explicit config path,
        // preserving the original single-argument console behavior.
        other => Command::Run {
            config_path: Some(PathBuf::from(other)),
        },
    }
}

/// Parse the tail after the `service` keyword.
fn parse_service(rest: &[String]) -> Command {
    match rest.first().map(String::as_str) {
        Some("run") => Command::ServiceRun,
        Some("install") => Command::ServiceInstall {
            name: parse_name(&rest[1..]),
        },
        Some("uninstall") => Command::ServiceUninstall {
            name: parse_name(&rest[1..]),
        },
        Some(other) => Command::Unknown(format!("service {other}")),
        None => Command::Unknown("service".to_owned()),
    }
}

/// Scan `args` for `--name NAME`, returning the value if present.
fn parse_name(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if arg == "--name" {
            return it.next().cloned();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_args_is_console_run() {
        assert_eq!(parse_args(&[]), Command::Run { config_path: None });
    }

    #[test]
    fn run_keyword_is_console_run() {
        assert_eq!(
            parse_args(&args(&["run"])),
            Command::Run { config_path: None }
        );
    }

    #[test]
    fn run_keyword_takes_optional_config_path() {
        assert_eq!(
            parse_args(&args(&["run", "C:\\cfg.json"])),
            Command::Run {
                config_path: Some(PathBuf::from("C:\\cfg.json")),
            }
        );
    }

    #[test]
    fn bare_path_is_console_run_with_that_path() {
        assert_eq!(
            parse_args(&args(&["C:\\cfg.json"])),
            Command::Run {
                config_path: Some(PathBuf::from("C:\\cfg.json")),
            }
        );
    }

    #[test]
    fn service_run_is_service_run() {
        assert_eq!(parse_args(&args(&["service", "run"])), Command::ServiceRun);
    }

    #[test]
    fn service_install_without_name() {
        assert_eq!(
            parse_args(&args(&["service", "install"])),
            Command::ServiceInstall { name: None }
        );
    }

    #[test]
    fn service_install_with_name() {
        assert_eq!(
            parse_args(&args(&["service", "install", "--name", "Zicade-Test"])),
            Command::ServiceInstall {
                name: Some("Zicade-Test".to_owned()),
            }
        );
    }

    #[test]
    fn service_uninstall_without_name() {
        assert_eq!(
            parse_args(&args(&["service", "uninstall"])),
            Command::ServiceUninstall { name: None }
        );
    }

    #[test]
    fn service_uninstall_with_name() {
        assert_eq!(
            parse_args(&args(&["service", "uninstall", "--name", "Zicade-Test"])),
            Command::ServiceUninstall {
                name: Some("Zicade-Test".to_owned()),
            }
        );
    }

    #[test]
    fn help_flags() {
        assert_eq!(parse_args(&args(&["--help"])), Command::Help);
        assert_eq!(parse_args(&args(&["-h"])), Command::Help);
        assert_eq!(parse_args(&args(&["help"])), Command::Help);
    }

    #[test]
    fn unknown_service_subcommand() {
        assert_eq!(
            parse_args(&args(&["service", "frobnicate"])),
            Command::Unknown("service frobnicate".to_owned())
        );
    }

    #[test]
    fn bare_service_is_unknown() {
        assert_eq!(
            parse_args(&args(&["service"])),
            Command::Unknown("service".to_owned())
        );
    }

    #[test]
    fn unknown_leading_flag() {
        assert_eq!(
            parse_args(&args(&["--nope"])),
            Command::Unknown("--nope".to_owned())
        );
    }
}
