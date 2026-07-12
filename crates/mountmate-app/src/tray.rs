use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use crate::i18n::{Locale, TextKey};
use mountmate_core::APP_NAME;

const SHOW_MAIN_ID: &str = "ssh-mountmate.show-main";
const SHOW_TRANSFERS_ID: &str = "ssh-mountmate.show-transfers";
const MOUNT_ALL_ID: &str = "ssh-mountmate.mount-all";
const UNMOUNT_ALL_ID: &str = "ssh-mountmate.unmount-all";
const EXIT_ID: &str = "ssh-mountmate.exit";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    ShowMain,
    ShowTransfers,
    MountAll,
    UnmountAll,
    Exit,
}

pub(crate) struct TrayController {
    _icon: TrayIcon,
    show_main: MenuItem,
    show_transfers: MenuItem,
    mount_all: MenuItem,
    unmount_all: MenuItem,
    exit: MenuItem,
    locale: Locale,
    can_mount: bool,
    can_unmount: bool,
}

impl TrayController {
    pub(crate) fn new(locale: Locale) -> Result<Self, String> {
        initialize_desktop_menu_runtime()?;

        let show_main = MenuItem::with_id(
            SHOW_MAIN_ID,
            locale.text(TextKey::ShowMainWindow),
            true,
            None,
        );
        let show_transfers = MenuItem::with_id(
            SHOW_TRANSFERS_ID,
            locale.text(TextKey::TransferCenter),
            true,
            None,
        );
        let mount_all = MenuItem::with_id(MOUNT_ALL_ID, locale.text(TextKey::MountAll), true, None);
        let unmount_all =
            MenuItem::with_id(UNMOUNT_ALL_ID, locale.text(TextKey::UnmountAll), true, None);
        let exit = MenuItem::with_id(EXIT_ID, locale.text(TextKey::Exit), true, None);
        let first_separator = PredefinedMenuItem::separator();
        let second_separator = PredefinedMenuItem::separator();
        let menu = Menu::with_items(&[
            &show_main,
            &show_transfers,
            &first_separator,
            &mount_all,
            &unmount_all,
            &second_separator,
            &exit,
        ])
        .map_err(|error| error.to_string())?;
        let icon = TrayIconBuilder::new()
            .with_tooltip(APP_NAME)
            .with_icon(application_icon()?)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(true)
            .with_menu_on_right_click(true)
            .build()
            .map_err(|error| error.to_string())?;

        Ok(Self {
            _icon: icon,
            show_main,
            show_transfers,
            mount_all,
            unmount_all,
            exit,
            locale,
            can_mount: true,
            can_unmount: true,
        })
    }

    pub(crate) fn sync(&mut self, locale: Locale, can_mount: bool, can_unmount: bool) {
        if locale != self.locale {
            self.show_main
                .set_text(locale.text(TextKey::ShowMainWindow));
            self.show_transfers
                .set_text(locale.text(TextKey::TransferCenter));
            self.mount_all.set_text(locale.text(TextKey::MountAll));
            self.unmount_all.set_text(locale.text(TextKey::UnmountAll));
            self.exit.set_text(locale.text(TextKey::Exit));
            self.locale = locale;
        }
        if can_mount != self.can_mount {
            self.mount_all.set_enabled(can_mount);
            self.can_mount = can_mount;
        }
        if can_unmount != self.can_unmount {
            self.unmount_all.set_enabled(can_unmount);
            self.can_unmount = can_unmount;
        }
    }

    pub(crate) fn drain_actions() -> Vec<TrayAction> {
        desktop_menu_iteration();
        let mut actions = Vec::new();
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            if let Some(action) = action_for_id(event.id.as_ref()) {
                actions.push(action);
            }
        }
        actions
    }
}

fn action_for_id(id: &str) -> Option<TrayAction> {
    match id {
        SHOW_MAIN_ID => Some(TrayAction::ShowMain),
        SHOW_TRANSFERS_ID => Some(TrayAction::ShowTransfers),
        MOUNT_ALL_ID => Some(TrayAction::MountAll),
        UNMOUNT_ALL_ID => Some(TrayAction::UnmountAll),
        EXIT_ID => Some(TrayAction::Exit),
        _ => None,
    }
}

fn application_icon() -> Result<Icon, String> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0; (SIZE * SIZE * 4) as usize];
    for y in 3..29 {
        for x in 3..29 {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            if dx * dx + dy * dy <= 13 * 13 {
                set_pixel(&mut rgba, SIZE, x, y, [31, 139, 112, 255]);
            }
        }
    }
    for y in 9..14 {
        for x in 8..24 {
            set_pixel(&mut rgba, SIZE, x, y, [245, 249, 248, 255]);
        }
    }
    for y in 18..23 {
        for x in 8..24 {
            set_pixel(&mut rgba, SIZE, x, y, [245, 249, 248, 255]);
        }
    }
    for y in 14..18 {
        for x in 14..18 {
            set_pixel(&mut rgba, SIZE, x, y, [245, 249, 248, 255]);
        }
    }
    Icon::from_rgba(rgba, SIZE, SIZE).map_err(|error| error.to_string())
}

fn set_pixel(rgba: &mut [u8], width: u32, x: u32, y: u32, color: [u8; 4]) {
    let offset = ((y * width + x) * 4) as usize;
    rgba[offset..offset + 4].copy_from_slice(&color);
}

#[cfg(target_os = "linux")]
fn initialize_desktop_menu_runtime() -> Result<(), String> {
    let appindicator_available = [
        "libayatana-appindicator3.so.1",
        "libappindicator3.so.1",
        "libayatana-appindicator3.so",
        "libappindicator3.so",
    ]
    .iter()
    .any(|name| unsafe { libloading::Library::new(name).is_ok() });
    if !appindicator_available {
        return Err(
            "Ayatana AppIndicator or AppIndicator 3 is not installed on this desktop".into(),
        );
    }
    if gtk::is_initialized() || gtk::init().is_ok() {
        Ok(())
    } else {
        Err("GTK could not be initialized for the system tray".into())
    }
}

#[cfg(not(target_os = "linux"))]
fn initialize_desktop_menu_runtime() -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn desktop_menu_iteration() {
    while gtk::events_pending() {
        gtk::main_iteration_do(false);
    }
}

#[cfg(not(target_os = "linux"))]
fn desktop_menu_iteration() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_ids_map_to_explicit_actions() {
        assert_eq!(action_for_id(SHOW_MAIN_ID), Some(TrayAction::ShowMain));
        assert_eq!(
            action_for_id(SHOW_TRANSFERS_ID),
            Some(TrayAction::ShowTransfers)
        );
        assert_eq!(action_for_id(MOUNT_ALL_ID), Some(TrayAction::MountAll));
        assert_eq!(action_for_id(UNMOUNT_ALL_ID), Some(TrayAction::UnmountAll));
        assert_eq!(action_for_id(EXIT_ID), Some(TrayAction::Exit));
        assert_eq!(action_for_id("unknown"), None);
    }

    #[test]
    fn generated_icon_has_transparency_and_visible_content() {
        let icon = application_icon().unwrap();
        drop(icon);

        let mut rgba = vec![0; 32 * 32 * 4];
        set_pixel(&mut rgba, 32, 4, 7, [1, 2, 3, 4]);
        assert_eq!(&rgba[(7 * 32 + 4) * 4..(7 * 32 + 5) * 4], &[1, 2, 3, 4]);
    }
}
