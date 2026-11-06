use anyhow::{Result, bail};

pub const HELP: &str = "lyricflow - Terminal karaoke for Spotify

Usage: lyricflow [COMMAND]
       lyricflow [--help | --version]

Commands:
  (none)    Start karaoke
  login     Connect your Spotify account
  logout    Remove local Spotify credentials
  status    Show connection status and the current track
  settings  Open settings without connecting to Spotify

Options:
  -h, --help     Show this help
  -V, --version  Show the version

Settings: Esc saves and exits; Q exits.
Settings are stored in ~/.config/lyricflow/settings.json.
Logout removes credentials on this computer; it does not revoke Spotify access.";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Run,
    Login,
    Logout,
    Status,
    Settings,
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Command> {
    let args: Vec<String> = args.into_iter().collect();
    let [first, rest @ ..] = args.as_slice() else {
        return Ok(Command::Run);
    };
    let command = match first.as_str() {
        "login" => Command::Login,
        "logout" => Command::Logout,
        "status" => Command::Status,
        "settings" => Command::Settings,
        "--help" | "-h" => Command::Help,
        "--version" | "-V" => Command::Version,
        _ => bail!("Unknown command: {first}. Run `lyricflow --help` for usage."),
    };
    match rest {
        [] => Ok(command),
        [help]
            if matches!(
                command,
                Command::Login | Command::Logout | Command::Status | Command::Settings
            ) && matches!(help.as_str(), "--help" | "-h") =>
        {
            Ok(Command::Help)
        }
        _ => bail!("Unexpected arguments after {first}. Run `lyricflow --help` for usage."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command> {
        parse(args.iter().map(|arg| (*arg).to_owned()))
    }

    #[test]
    fn no_arguments_start_karaoke() {
        assert_eq!(parse_args(&[]).unwrap(), Command::Run);
    }

    #[test]
    fn parses_each_command_and_global_flag() {
        for (arg, expected) in [
            ("login", Command::Login),
            ("logout", Command::Logout),
            ("status", Command::Status),
            ("settings", Command::Settings),
            ("--help", Command::Help),
            ("-h", Command::Help),
            ("--version", Command::Version),
            ("-V", Command::Version),
        ] {
            assert_eq!(parse_args(&[arg]).unwrap(), expected, "{arg}");
        }
    }

    #[test]
    fn subcommand_help_does_not_execute_the_command() {
        for command in ["login", "logout", "status", "settings"] {
            for flag in ["--help", "-h"] {
                assert_eq!(parse_args(&[command, flag]).unwrap(), Command::Help);
            }
        }
    }

    #[test]
    fn rejects_unknown_commands_and_flags_with_usage_guidance() {
        for arg in ["unknown", "--unknown", "-v", "LOGIN", "run", ""] {
            let error = parse_args(&[arg]).unwrap_err().to_string();
            assert!(error.contains("Unknown command:"));
            assert!(error.contains("lyricflow --help"));
        }
        assert!(parse_args(&["unknown", "--help"]).is_err());
    }

    #[test]
    fn rejects_extra_arguments_instead_of_silently_running_a_command() {
        for args in [
            vec!["login", "extra"],
            vec!["logout", "--force"],
            vec!["status", "settings"],
            vec!["settings", "--unknown"],
            vec!["login", "--help", "extra"],
            vec!["--help", "--version"],
            vec!["--version", "extra"],
        ] {
            let error = parse_args(&args).unwrap_err().to_string();
            assert!(error.contains("Unexpected arguments"), "{args:?}");
        }
    }

    #[test]
    fn help_lists_every_supported_command_and_settings_controls() {
        for text in [
            "login",
            "logout",
            "status",
            "settings",
            "--help",
            "--version",
            "Esc saves and exits",
            "Q exits",
            "~/.config/lyricflow/settings.json",
        ] {
            assert!(HELP.contains(text), "Missing help: {text}");
        }
    }
}
