use std::path::PathBuf;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use mountmate_core::navigation_refresh::NavigationEvent;

/// A cloneable receiver backed by a dedicated observer thread.  The app
/// consumes it through an iced subscription and never blocks its UI thread.
#[derive(Clone)]
pub struct NavigationObserver {
    receiver: async_channel::Receiver<NavigationEvent>,
    failure: Arc<Mutex<Option<String>>>,
}

impl NavigationObserver {
    pub async fn recv(&self) -> Result<NavigationEvent, async_channel::RecvError> {
        self.receiver.recv().await
    }

    pub fn failure(&self) -> Option<String> {
        self.failure.lock().ok().and_then(|failure| failure.clone())
    }
}

impl Hash for NavigationObserver {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "ssh-mountmate-explorer-navigation-observer".hash(state);
    }
}

pub fn start_navigation_observer() -> Result<NavigationObserver, String> {
    #[cfg(windows)]
    {
        let (sender, receiver) = async_channel::bounded(64);
        let failure = Arc::new(Mutex::new(None));
        let thread_failure = failure.clone();
        std::thread::Builder::new()
            .name("ssh-mountmate-explorer-observer".into())
            .spawn(move || poll_explorer_windows(sender, thread_failure))
            .map_err(|error| error.to_string())?;
        return Ok(NavigationObserver { receiver, failure });
    }
    #[cfg(not(windows))]
    {
        Err("Explorer navigation observation is available in the Windows installed edition only".into())
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
        CoUninitialize,
    };
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::{IWebBrowserApp, IShellWindows, ShellWindows};

    // Explorer's automation objects are apartment-bound.  Keep all COM work
    // on this thread and reconcile paths at a low frequency; no DLL injection
    // or UI/address-bar scraping is involved.
    let init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if init.is_err() {
        set_failure(&failure, "COM apartment initialization failed");
        return;
    }
    let shell: IShellWindows = match unsafe { CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER) } {
        Ok(shell) => shell,
        Err(_) => {
            set_failure(&failure, "Explorer automation is unavailable");
            unsafe { CoUninitialize() };
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
            let variant = VARIANT::from(index);
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
            let id = hwnd as u64;
            observed.insert(id, path_key.clone());
            if previous.get(&id) != Some(&path_key)
                && sender
                    .send_blocking(NavigationEvent {
                        window_id: id,
                        target: path,
                    })
                    .is_err()
            {
                unsafe { CoUninitialize() };
                return;
            }
        }
        previous = observed;
        std::thread::sleep(Duration::from_millis(750));
    }
    unsafe { CoUninitialize() };
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
    use windows_sys::Win32::UI::Shell::{SHChangeNotify, SHCNE_UPDATEDIR, SHCNF_PATHW};

    let wide = path.as_os_str().encode_wide().chain(Some(0)).collect::<Vec<_>>();
    unsafe {
        SHChangeNotify(
            SHCNE_UPDATEDIR,
            SHCNF_PATHW,
            wide.as_ptr().cast(),
            std::ptr::null(),
        );
    }
}

#[cfg(not(windows))]
pub fn notify_shell_updated_dir(_path: &std::path::Path) {}
