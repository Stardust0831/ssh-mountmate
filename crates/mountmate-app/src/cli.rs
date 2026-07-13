use std::path::PathBuf;

use mountmate_core::app_command::AppCommand;
use mountmate_core::update_helper::{UpdateHealthAuthorization, UpdateHelperAuthorization};
use mountmate_core::{APP_NAME, VERSION};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchAction {
    Gui {
        command: AppCommand,
        update_health: Option<UpdateHealthAuthorization>,
    },
    Headless(AppCommand),
    RunUpdateHelper(UpdateHelperAuthorization),
    CheckUpdate,
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
        return Ok(LaunchAction::Gui {
            command: AppCommand::ShowMain,
            update_health: None,
        });
    }
    let mut action = None;
    let mut relative_dir = String::new();
    let mut update_helper_token = None;
    let mut update_health_marker = None;
    let mut update_health_token = None;
    let mut index = 0;
    while index < arguments.len() {
        let argument = &arguments[index];
        let candidate = match argument.as_str() {
            "-h" | "--help" => Some(LaunchAction::Help),
            "-V" | "--version" => Some(LaunchAction::Version),
            "--licenses" => Some(LaunchAction::Licenses),
            "--check-update" => Some(LaunchAction::CheckUpdate),
            "--register-file-manager-menu" | "--register-shell-menu" => {
                Some(LaunchAction::RegisterFileManagerMenu)
            }
            "--unregister-file-manager-menu" | "--unregister-shell-menu" => {
                Some(LaunchAction::UnregisterFileManagerMenu)
            }
            "--show-main" => Some(LaunchAction::Gui {
                command: AppCommand::ShowMain,
                update_health: None,
            }),
            "--show-transfers" => Some(LaunchAction::Gui {
                command: AppCommand::ShowTransfers,
                update_health: None,
            }),
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
            "--run-update-helper" => {
                Some(LaunchAction::RunUpdateHelper(UpdateHelperAuthorization {
                    plan_path: PathBuf::from(next_value(&arguments, &mut index, argument)?),
                    token: String::new(),
                }))
            }
            "--update-helper-token" => {
                set_once(
                    &mut update_helper_token,
                    next_value(&arguments, &mut index, argument)?,
                    argument,
                )?;
                None
            }
            "--update-health-marker" => {
                set_once(
                    &mut update_health_marker,
                    PathBuf::from(next_value(&arguments, &mut index, argument)?),
                    argument,
                )?;
                None
            }
            "--update-health-token" => {
                set_once(
                    &mut update_health_token,
                    next_value(&arguments, &mut index, argument)?,
                    argument,
                )?;
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
    match &mut action {
        LaunchAction::RunUpdateHelper(authorization) => {
            authorization.token = update_helper_token
                .ok_or_else(|| "--run-update-helper requires --update-helper-token".to_owned())?;
            if update_health_marker.is_some() || update_health_token.is_some() {
                return Err("update health arguments cannot be used by the update helper".into());
            }
        }
        LaunchAction::Gui { update_health, .. } => {
            if update_helper_token.is_some() {
                return Err("--update-helper-token requires --run-update-helper".into());
            }
            *update_health = match (update_health_marker, update_health_token) {
                (None, None) => None,
                (Some(marker_path), Some(token)) => {
                    Some(UpdateHealthAuthorization { marker_path, token })
                }
                _ => {
                    return Err(
                        "--update-health-marker and --update-health-token must be used together"
                            .into(),
                    );
                }
            };
        }
        _ => {
            if update_helper_token.is_some()
                || update_health_marker.is_some()
                || update_health_token.is_some()
            {
                return Err("internal update arguments require their matching command".into());
            }
        }
    }
    Ok(action)
}

fn set_once<T>(slot: &mut Option<T>, value: T, option: &str) -> Result<(), String> {
    if slot.replace(value).is_some() {
        Err(format!("{option} can only be used once"))
    } else {
        Ok(())
    }
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
  --check-update                 Check GitHub for a verified platform update
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
        "\n\n--- windows-rs license ---\n\n",
        include_str!("../../../licenses/windows-LICENSE-MIT.txt"),
        "\n\n--- notify-rust license ---\n\n",
        include_str!("../../../licenses/notify-rust-LICENSE-MIT.txt"),
        "\n\n--- tauri-winrt-notification license ---\n\n",
        include_str!("../../../licenses/tauri-winrt-notification-LICENSE-MIT.txt"),
        "\n\n--- semver license ---\n\n",
        include_str!("../../../licenses/semver-LICENSE-MIT.txt"),
        "\n\n--- zip license ---\n\n",
        include_str!("../../../licenses/zip-LICENSE-MIT.txt"),
        "\n\n--- rustix license ---\n\n",
        include_str!("../../../licenses/rustix-LICENSE-MIT.txt"),
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
            LaunchAction::Gui {
                command: AppCommand::ShowTransfers,
                update_health: None,
            }
        );
        assert_eq!(
            parse(args(&["--register-shell-menu"])).unwrap(),
            LaunchAction::RegisterFileManagerMenu
        );
        assert_eq!(
            parse(args(&["--check-update"])).unwrap(),
            LaunchAction::CheckUpdate
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
        assert!(help().contains("\n  --check-update"));
        assert!(help().contains("\n  -V, --version"));
        assert!(!help().contains("update-helper"));
        assert!(!help().contains("update-health"));
    }

    #[test]
    fn internal_update_commands_require_complete_paired_authorization() {
        assert!(matches!(
            parse(args(&[
                "--run-update-helper",
                "/state/plan.json",
                "--update-helper-token",
                "secret"
            ])),
            Ok(LaunchAction::RunUpdateHelper(_))
        ));
        assert!(parse(args(&["--run-update-helper", "/state/plan.json"])).is_err());
        assert!(
            parse(args(&[
                "--show-main",
                "--update-health-marker",
                "/state/health.json"
            ]))
            .is_err()
        );
        assert!(
            parse(args(&[
                "--show-main",
                "--update-health-marker",
                "/state/health.json",
                "--update-health-token",
                "secret"
            ]))
            .is_ok()
        );
    }
}
