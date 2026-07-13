#[cfg(target_os = "linux")]
mod linux {
    use std::collections::HashMap;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use zbus::zvariant::OwnedValue;

    pub struct Notifications {
        marker: PathBuf,
        next_id: AtomicU32,
    }

    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Notifications {
        fn get_capabilities(&self) -> Vec<String> {
            vec!["body".into()]
        }

        #[allow(clippy::too_many_arguments)] // Freedesktop Notify has this fixed D-Bus signature.
        fn notify(
            &self,
            app_name: String,
            replaces_id: u32,
            app_icon: String,
            summary: String,
            body: String,
            actions: Vec<String>,
            hints: HashMap<String, OwnedValue>,
            expire_timeout: i32,
        ) -> u32 {
            let id = if replaces_id == 0 {
                self.next_id.fetch_add(1, Ordering::Relaxed)
            } else {
                replaces_id
            };
            let mut marker = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.marker)
                .expect("notification marker should be writable");
            writeln!(
                marker,
                "id={id}\tapp={app_name}\ticon={app_icon}\tsummary={summary}\tbody={body}\tactions={}\thints={}\ttimeout={expire_timeout}",
                actions.len(),
                hints.len(),
            )
            .expect("notification marker should accept a record");
            id
        }

        fn close_notification(&self, _id: u32) {}

        fn get_server_information(&self) -> (String, String, String, String) {
            (
                "SSH MountMate notification probe".into(),
                "SSH MountMate".into(),
                env!("CARGO_PKG_VERSION").into(),
                "1.2".into(),
            )
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let marker = std::env::args_os()
            .nth(1)
            .map(PathBuf::from)
            .ok_or("usage: freedesktop_notification_probe MARKER")?;
        let _connection = zbus::blocking::connection::Builder::session()?
            .name("org.freedesktop.Notifications")?
            .serve_at(
                "/org/freedesktop/Notifications",
                Notifications {
                    marker,
                    next_id: AtomicU32::new(1),
                },
            )?
            .build()?;
        loop {
            std::thread::park();
        }
    }
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("The Freedesktop notification probe is only available on Linux");
    std::process::exit(1);
}
