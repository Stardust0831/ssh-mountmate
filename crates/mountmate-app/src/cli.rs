use mountmate_core::app_command::AppCommand;
use mountmate_core::{APP_NAME, VERSION};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchAction {
    Gui(AppCommand),
    Headless(AppCommand),
    RegisterFileManagerMenu,
    UnregisterFileManagerMenu,
    Help,
    Version,
    Licenses,
}

pub(crate) fn parse(arguments: impl IntoIterator<Item = String>) -> Result<LaunchAction, String> {
    let arguments: Vec<_> = arguments
        .into_iter()
        .filter(|argument| !argument.starts_with("-psn_"))
        .collect();
    if arguments.is_empty() {
        return Ok(LaunchAction::Gui(AppCommand::ShowMain));
    }
    let mut action = None;
    let mut relative_dir = String::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let candidate = match argument.as_str() {
            "-h" | "--help" => Some(LaunchAction::Help),
            "-V" | "--version" => Some(LaunchAction::Version),
            "--licenses" => Some(LaunchAction::Licenses),
            "--register-file-manager-menu" | "--register-shell-menu" => {
                Some(LaunchAction::RegisterFileManagerMenu)
            }
            "--unregister-file-manager-menu" | "--unregister-shell-menu" => {
                Some(LaunchAction::UnregisterFileManagerMenu)
            }
            "--show-main" => Some(LaunchAction::Gui(AppCommand::ShowMain)),
            "--show-transfers" => Some(LaunchAction::Gui(AppCommand::ShowTransfers)),
            "--mount-all" | "--mount-startup-all" => {
                Some(LaunchAction::Headless(AppCommand::MountAll))
            }
            "--unmount-all" => Some(LaunchAction::Headless(AppCommand::UnmountAll)),
            "--mount-id" => Some(LaunchAction::Headless(AppCommand::Mount {
                id: next_value(&arguments, &mut index, argument)?,
            })),
            "--unmount-id" => Some(LaunchAction::Headless(AppCommand::Unmount {
                id: next_value(&arguments, &mut index, argument)?,
            })),
            "--open-id" => Some(LaunchAction::Headless(AppCommand::Open {
                id: next_value(&arguments, &mut index, argument)?,
            })),
            "--refresh-path" => Some(LaunchAction::Headless(AppCommand::RefreshPath {
                path: next_value(&arguments, &mut index, argument)?,
            })),
            "--refresh-id" => Some(LaunchAction::Headless(AppCommand::Refresh {
                id: next_value(&arguments, &mut index, argument)?,
                relative_dir: String::new(),
            })),
            "--relative-dir" => {
                relative_dir = next_value(&arguments, &mut index, argument)?;
                None
            }
            _ => return Err(format!("unknown argument: {argument}")),
        };
        if let Some(candidate) = candidate {
            if action.is_some() {
                return Err("only one command can be used at a time".into());
            }
            action = Some(candidate);
        }
        index += 1;
    }
    let mut action = action.ok_or_else(|| "no command was provided".to_owned())?;
    if !relative_dir.is_empty() {
        match &mut action {
            LaunchAction::Headless(AppCommand::Refresh {
                relative_dir: selected,
                ..
            }) => *selected = relative_dir,
            _ => return Err("--relative-dir requires --refresh-id".into()),
        }
    }
    Ok(action)
}

fn next_value(arguments: &[String], index: &mut usize, option: &str) -> Result<String, String> {
    *index += 1;
    arguments
        .get(*index)
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
        .cloned()
        .ok_or_else(|| format!("{option} requires a value"))
}

pub(crate) fn help() -> String {
    format!(
        r#"{APP_NAME} {VERSION}

Usage: SSHMountMate [COMMAND]

  --show-main                    Activate the main window
  --show-transfers               Open the transfer center
  --mount-id ID                  Mount one saved connection
  --unmount-id ID                Unmount one saved connection
  --open-id ID                   Open one mounted connection
  --mount-all                    Mount all saved connections concurrently
  --mount-startup-all            Compatibility alias for --mount-all
  --unmount-all                  Unmount all mounted connections concurrently
  --refresh-id ID                Refresh one mounted connection
  --relative-dir PATH            Directory used with --refresh-id
  --refresh-path PATH            Refresh the mount containing a local directory
  --register-file-manager-menu   Register file-manager commands for this executable
  --unregister-file-manager-menu Remove file-manager commands
  --licenses                     Print bundled third-party notices
  -h, --help                     Print this help
  -V, --version                  Print the version"#
    )
}

pub(crate) fn licenses() -> &'static str {
    concat!(
        include_str!("../../../THIRD_PARTY_NOTICES.md"),
        "\n\n--- rclone license ---\n\n",
        include_str!("../../../licenses/rclone-COPYING.txt"),
        "\n\n--- rfd license ---\n\n",
        include_str!("../../../licenses/rfd-LICENSE.txt"),
        "\n\n--- sys-locale license ---\n\n",
        include_str!("../../../licenses/sys-locale-LICENSE-MIT.txt"),
        "\n\n--- tray-icon and muda license ---\n\n",
        include_str!("../../../licenses/tray-icon-LICENSE-MIT.txt"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn historical_commands_remain_compatible() {
        assert_eq!(
            parse(args(&["--mount-id", "alpha"])).unwrap(),
            LaunchAction::Headless(AppCommand::Mount { id: "alpha".into() })
        );
        assert_eq!(
            parse(args(&["--mount-startup-all"])).unwrap(),
            LaunchAction::Headless(AppCommand::MountAll)
        );
        assert_eq!(
            parse(args(&["--show-transfers"])).unwrap(),
            LaunchAction::Gui(AppCommand::ShowTransfers)
        );
        assert_eq!(
            parse(args(&["--register-shell-menu"])).unwrap(),
            LaunchAction::RegisterFileManagerMenu
        );
    }

    #[test]
    fn refresh_id_accepts_one_relative_directory() {
        assert_eq!(
            parse(args(&[
                "--refresh-id",
                "alpha",
                "--relative-dir",
                "folder/child"
            ]))
            .unwrap(),
            LaunchAction::Headless(AppCommand::Refresh {
                id: "alpha".into(),
                relative_dir: "folder/child".into(),
            })
        );
    }

    #[test]
    fn conflicting_and_incomplete_commands_are_rejected() {
        assert!(parse(args(&["--mount-id"])).is_err());
        assert!(parse(args(&["--mount-all", "--show-main"])).is_err());
        assert!(parse(args(&["--relative-dir", "folder"])).is_err());
        assert!(parse(args(&["--unknown"])).is_err());
    }

    #[test]
    fn help_keeps_command_columns_readable() {
        assert!(help().contains("\n  --show-main"));
        assert!(help().contains("\n  -V, --version"));
    }
}
