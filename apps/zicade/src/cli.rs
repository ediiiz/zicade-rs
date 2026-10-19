//! Command-line argument parsing for the Zicade binary.
//!
//! Parsing is a **pure** function ([`parse_args`]) over an argument slice
//! (the process args with `argv[0]` already stripped), returning a [`Command`].
//! Keeping it free of any side effects — no runtime, no I/O — makes the whole
//! dispatch table unit-testable without a desktop session.

use std::path::PathBuf;

/// A parsed top-level command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// No subcommand at all (bare `zicade`). This — and only this — branches on
    /// the double-click heuristic ([`launch_mode`]): console when launched from
    /// a terminal, tray when double-clicked. An explicit `run` is [`Run`] and
    /// always stays console.
    Default,
    /// Console mode: serve until Ctrl-C. An optional explicit config path
    /// preserves the historical "first arg is the config path" behavior.
    Run { config_path: Option<PathBuf> },
    /// Tray mode without opening the window (`--background`), for start on
    /// login.
    Background,
    /// Print usage text (`--help`/`-h`/`help`).
    Help,
    /// An unrecognized command; carries the offending token(s) for the message.
    Unknown(String),
}

/// How the process runs: in the console, or in the tray with the window open
/// (a double-click) or closed (`--background`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Serve in the console until Ctrl-C (the historical behavior).
    Console,
    /// Hide the console, run with a tray icon, and open the window.
    Tray,
    /// Hide the console and run with a tray icon; the window stays closed
    /// until opened from the tray.
    Background,
}

/// Decide the launch mode from the number of processes attached to this
/// process's console and the parsed [`Command`].
///
/// Pure and total so the whole dispatch rule is unit-testable without a desktop
/// session. `console_process_count` is `GetConsoleProcessList`'s result: `1`
/// means we own a freshly-created console (a double-click) and the *default*
/// command runs in the tray; any other count, or any other explicit
/// subcommand, stays in the console. `--background` always runs in the tray.
pub fn launch_mode(console_process_count: usize, command: &Command) -> LaunchMode {
    match command {
        Command::Background => LaunchMode::Background,
        Command::Default if console_process_count == 1 => LaunchMode::Tray,
        _ => LaunchMode::Console,
    }
}

/// Parse `args` (process args with `argv[0]` stripped) into a [`Command`].
pub fn parse_args(args: &[String]) -> Command {
    let Some(first) = args.first() else {
        return Command::Default;
    };
    match first.as_str() {
        "--help" | "-h" | "help" => Command::Help,
        "--background" => Command::Background,
        "run" => Command::Run {
            config_path: args.get(1).map(PathBuf::from),
        },
        // Service mode is gone; reject it so a stale shortcut fails loudly
        // instead of serving with a config file named "service".
        "service" => Command::Unknown(args.join(" ")),
        // A leading flag we do not recognize is an error...
        other if other.starts_with('-') => Command::Unknown(other.to_owned()),
        // ...but any other bare token is treated as an explicit config path,
        // preserving the original single-argument console behavior.
        other => Command::Run {
            config_path: Some(PathBuf::from(other)),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_args_is_the_default_command() {
        // Bare `zicade` (double-clicked or typed) is the no-subcommand default,
        // which — and only which — branches on the double-click heuristic.
        assert_eq!(parse_args(&[]), Command::Default);
    }

    #[test]
    fn default_with_single_console_process_is_tray() {
        // GetConsoleProcessList == 1 means we own a freshly-created console
        // (double-click); the default command then runs in tray mode.
        assert_eq!(launch_mode(1, &Command::Default), LaunchMode::Tray);
    }

    #[test]
    fn default_with_multiple_console_processes_is_console() {
        // Launched from an existing cmd/PowerShell: the console is shared, so
        // GetConsoleProcessList > 1 and we stay in console mode.
        assert_eq!(launch_mode(2, &Command::Default), LaunchMode::Console);
        assert_eq!(launch_mode(3, &Command::Default), LaunchMode::Console);
    }

    #[test]
    fn default_with_zero_console_processes_is_console() {
        // No console attached at all (e.g. off-Windows or a detached process):
        // never tray — fall back to console.
        assert_eq!(launch_mode(0, &Command::Default), LaunchMode::Console);
    }

    #[test]
    fn explicit_subcommands_never_enter_tray_regardless_of_count() {
        // Only the no-subcommand default branches on the heuristic. Every
        // explicit command stays console even when the count says "double-click".
        for command in [
            Command::Run { config_path: None },
            Command::Run {
                config_path: Some(PathBuf::from("C:\\cfg.json")),
            },
            Command::Help,
            Command::Unknown("--nope".to_owned()),
        ] {
            assert_eq!(
                launch_mode(1, &command),
                LaunchMode::Console,
                "explicit command entered tray mode: {command:?}"
            );
        }
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
    fn removed_service_commands_are_rejected_not_read_as_a_config_path() {
        // Old shortcuts and scripts still run `zicade service run`; that must
        // fail loudly rather than serve with a config file named "service".
        assert_eq!(
            parse_args(&args(&["service", "run"])),
            Command::Unknown("service run".to_owned())
        );
        assert_eq!(
            parse_args(&args(&["service"])),
            Command::Unknown("service".to_owned())
        );
    }

    #[test]
    fn background_flag_starts_in_the_tray_without_a_window() {
        // Start-on-login uses this: the proxy must come up, but popping the
        // window at every sign-in would be noise.
        assert_eq!(parse_args(&args(&["--background"])), Command::Background);
        assert_eq!(launch_mode(0, &Command::Background), LaunchMode::Background);
        assert_eq!(launch_mode(2, &Command::Background), LaunchMode::Background);
    }

    #[test]
    fn help_flags() {
        assert_eq!(parse_args(&args(&["--help"])), Command::Help);
        assert_eq!(parse_args(&args(&["-h"])), Command::Help);
        assert_eq!(parse_args(&args(&["help"])), Command::Help);
    }

    #[test]
    fn unknown_leading_flag() {
        assert_eq!(
            parse_args(&args(&["--nope"])),
            Command::Unknown("--nope".to_owned())
        );
    }
}
