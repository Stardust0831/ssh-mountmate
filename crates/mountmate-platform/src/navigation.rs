use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

#[cfg(windows)]
use std::path::PathBuf;
#[cfg(windows)]
use std::sync::atomic::{AtomicU64, Ordering};

use mountmate_core::navigation_refresh::NavigationEvent;

/// A cloneable receiver backed by a dedicated observer thread.  The app
/// consumes it through an iced subscription and never blocks its UI thread.
#[derive(Clone)]
pub struct NavigationObserver {
    subscription_id: u64,
    receiver: async_channel::Receiver<NavigationEvent>,
    failure: Arc<Mutex<Option<String>>>,
}

impl NavigationObserver {
    pub async fn recv(&self) -> Result<NavigationEvent, async_channel::RecvError> {
        self.receiver.recv().await
    }

    pub fn events(&self) -> async_channel::Receiver<NavigationEvent> {
        self.receiver.clone()
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }
}

impl Hash for NavigationObserver {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.subscription_id.hash(state);
    }
}

pub fn start_navigation_observer() -> Result<NavigationObserver, String> {
    #[cfg(windows)]
    {
        static NEXT_SUBSCRIPTION_ID: AtomicU64 = AtomicU64::new(1);
        let (sender, receiver) = async_channel::bounded(64);
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        std::thread::Builder::new()
            .name("ssh-mountmate-explorer-observer".into())
            .spawn(move || poll_explorer_windows(sender, thread_failure))
            .map_err(|error| error.to_string())?;
        return Ok(NavigationObserver {
            subscription_id: NEXT_SUBSCRIPTION_ID.fetch_add(1, Ordering::Relaxed),
            receiver,
            failure,
        });
    }
    #[cfg(not(windows))]
    {
        Err(
            "Explorer navigation observation is available in the Windows installed edition only"
                .into(),
        )
    }
}

#[cfg(windows)]
fn poll_explorer_windows(
    sender: async_channel::Sender<NavigationEvent>,
    failure: Arc<Mutex<Option<String>>>,
) {
    use std::collections::HashMap;
    use std::time::Duration;

    use windows::Win32::System::Com::{
        CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{IShellWindows, IWebBrowserApp, ShellWindows};
    use windows::core::Interface;

    // Explorer's automation objects are apartment-bound.  Keep all COM work
    // on this thread and reconcile paths at a low frequency; no DLL injection
    // or UI/address-bar scraping is involved.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        set_failure(&failure, "COM apartment initialization failed");
        return;
    }
    let _apartment = ComApartment;
    let shell: IShellWindows =
        match unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER) } {
            Ok(shell) => shell,
            Err(_) => {
                set_failure(&failure, "Explorer automation is unavailable");
                return;
            }
        };
    let mut previous = HashMap::<u64, String>::new();
    loop {
        if sender.is_closed() {
            break;
        }
        let count = unsafe { shell.Count() }.unwrap_or(0);
        let mut observed = HashMap::new();
        for index in 0..count {
            let variant = i32_variant(index);
            let Ok(dispatch) = (unsafe { shell.Item(&variant) }) else {
                continue;
            };
            let Ok(browser) = dispatch.cast::<IWebBrowserApp>() else {
                continue;
            };
            let Ok(hwnd) = (unsafe { browser.HWND() }) else {
                continue;
            };
            let Ok(url) = (unsafe { browser.LocationURL() }) else {
                continue;
            };
            let path = match file_url_to_path(&url.to_string()) {
                Some(path) => path,
                None => continue,
            };
            if path.as_os_str().is_empty() {
                continue;
            }
            let path_key = path.to_string_lossy().into_owned();
            let id = hwnd.0 as u64;
            observed.insert(id, path_key.clone());
            if previous.get(&id) != Some(&path_key)
                && sender
                    .send_blocking(NavigationEvent {
                        window_id: id,
                        target: path,
                    })
                    .is_err()
            {
                return;
            }
        }
        previous = observed;
        std::thread::sleep(Duration::from_millis(750));
    }
}

#[cfg(windows)]
struct ComApartment;

#[cfg(windows)]
impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { windows::Win32::System::Com::CoUninitialize() };
    }
}

#[cfg(windows)]
fn i32_variant(value: i32) -> windows::Win32::System::Variant::VARIANT {
    use std::mem::ManuallyDrop;
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};

    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: VARIANT_0_0_0 { lVal: value },
            }),
        },
    }
}

#[cfg(windows)]
fn file_url_to_path(value: &str) -> Option<PathBuf> {
    let url = url::Url::parse(value).ok()?;
    if url.scheme() != "file" {
        return None;
    }
    url.to_file_path().ok()
}

#[cfg(windows)]
fn set_failure(failure: &Arc<Mutex<Option<String>>>, message: &str) {
    if let Ok(mut slot) = failure.lock() {
        *slot = Some(message.to_owned());
    }
}

#[cfg(windows)]
pub fn notify_shell_updated_dir(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{SHCNE_UPDATEDIR, SHCNF_PATHW, SHChangeNotify};

    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEDIR as i32,
            SHCNF_PATHW,
            wide.as_ptr().cast(),
            std::ptr::null(),
        );
    }
}

#[cfg(not(windows))]
pub fn notify_shell_updated_dir(_path: &std::path::Path) {}

#[cfg(test)]
mod tests {
    use std::collections::hash_map::DefaultHasher;

    use super::*;

    fn observer_hash(observer: &NavigationObserver) -> u64 {
        let mut hasher = DefaultHasher::new();
        observer.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn replacement_observers_have_distinct_subscription_identities() {
        let (_, first_receiver) = async_channel::bounded(1);
        let (_, second_receiver) = async_channel::bounded(1);
        let first = NavigationObserver {
            subscription_id: 1,
            receiver: first_receiver,
            failure: Arc::new(Mutex::new(None)),
        };
        let second = NavigationObserver {
            subscription_id: 2,
            receiver: second_receiver,
            failure: Arc::new(Mutex::new(None)),
        };

        assert_ne!(observer_hash(&first), observer_hash(&second));
    }
}
