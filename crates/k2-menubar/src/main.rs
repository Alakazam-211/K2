//! Bare Mach-O menu-bar helper. Launchd starts the copy in `~/.k2/bin`,
//! not a binary inside `K2.app` (that would adopt bundle id `dev.k2.app`).

#[cfg(target_os = "macos")]
mod macos {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy, NSEventMask};
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode};
    use tray_icon::menu::{Menu, MenuEvent, MenuItem};
    use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

    use k2_menubar::{
        probe_live, quit_daemon, status_label, DaemonState, POLL_INTERVAL, QUIT_LABEL,
        STATUS_NOT_RUNNING,
    };

    const QUIT_ID: &str = "k2.menubar.quit-daemon";
    /// Same PNG `src-tauri/src/tray.rs` embeds. `tray-icon` wants RGBA, and
    /// this binary does not link Tauri, so the bytes are decoded here.
    const TRAY_ICON_PNG: &[u8] = include_bytes!("../../../resources/tray-icon.png");

    struct Shared {
        epoch: AtomicU64,
        result: Mutex<Option<(u64, DaemonState)>>,
    }

    pub fn run() -> Result<(), String> {
        let mtm = MainThreadMarker::new().ok_or("k2-menubar must run on the main thread")?;
        let app = NSApplication::sharedApplication(mtm);
        // Accessory before the status-item event loop so there is no Dock icon.
        if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
            return Err("NSApplicationActivationPolicyAccessory was rejected".to_string());
        }
        app.finishLaunching();

        let (status, _tray) = build_tray()?;
        let shared = Arc::new(Shared {
            epoch: AtomicU64::new(0),
            result: Mutex::new(None),
        });
        let gate = Arc::new(Mutex::new(()));
        let (kick_tx, kick_rx) = mpsc::channel::<()>();
        spawn_probe(Arc::clone(&shared), Arc::clone(&gate), kick_rx);

        let mut shown = DaemonState::NotRunning;
        loop {
            if let Some((epoch, state)) = *shared.result.lock().expect("probe result") {
                if epoch == shared.epoch.load(Ordering::SeqCst) && state != shown {
                    status.set_text(status_label(state));
                    status.set_enabled(false);
                    shown = state;
                }
            }
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id().as_ref() == QUIT_ID {
                    if let Some(state) = handle_quit_daemon(&status, &shared, &gate, &kick_tx) {
                        shown = state;
                    }
                }
            }
            // Status-item clicks are app events. A bare NSRunLoop does not
            // deliver them, so the menu never opens.
            let until = NSDate::dateWithTimeIntervalSinceNow(0.05);
            while let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::Any,
                Some(&until),
                unsafe { NSDefaultRunLoopMode },
                true,
            ) {
                app.sendEvent(&event);
            }
        }
    }

    fn build_tray() -> Result<(MenuItem, TrayIcon), String> {
        let status = MenuItem::with_id("k2.menubar.status", STATUS_NOT_RUNNING, false, None);
        let quit = MenuItem::with_id(QUIT_ID, QUIT_LABEL, true, None);
        let menu = Menu::new();
        menu.append(&status)
            .map_err(|e| format!("status row: {e}"))?;
        menu.append(&quit).map_err(|e| format!("quit row: {e}"))?;

        let decoded = image::load_from_memory(TRAY_ICON_PNG)
            .map_err(|e| format!("decode tray icon: {e}"))?
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        let icon = Icon::from_rgba(decoded.into_raw(), width, height)
            .map_err(|e| format!("tray icon: {e}"))?;
        let tray = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_icon(icon)
            .with_icon_as_template(true)
            .with_tooltip("K2")
            .with_menu_on_left_click(true)
            .build()
            .map_err(|e| format!("build status item: {e}"))?;
        Ok((status, tray))
    }

    fn spawn_probe(shared: Arc<Shared>, gate: Arc<Mutex<()>>, kick_rx: mpsc::Receiver<()>) {
        std::thread::Builder::new()
            .name("k2-menubar-probe".to_string())
            .spawn(move || loop {
                {
                    let _hold = gate.lock().expect("probe gate");
                    let epoch = shared.epoch.load(Ordering::SeqCst);
                    let state = probe_live();
                    if shared.epoch.load(Ordering::SeqCst) == epoch {
                        *shared.result.lock().expect("probe result") = Some((epoch, state));
                    }
                }
                let _ = kick_rx.recv_timeout(POLL_INTERVAL);
                while kick_rx.try_recv().is_ok() {}
            })
            .expect("spawn probe");
    }

    /// Unload `dev.k2.daemon`. Does not exit this process.
    fn handle_quit_daemon(
        status: &MenuItem,
        shared: &Shared,
        gate: &Mutex<()>,
        kick_tx: &mpsc::Sender<()>,
    ) -> Option<DaemonState> {
        let stopped = {
            let _hold = gate.lock().expect("quit gate");
            shared.epoch.fetch_add(1, Ordering::SeqCst);
            *shared.result.lock().expect("probe result") = None;
            match quit_daemon() {
                Ok(()) => true,
                Err(e) => {
                    eprintln!("k2-menubar: quit daemon: {e}");
                    false
                }
            }
        };
        let _ = kick_tx.send(());
        if !stopped {
            return None;
        }
        let state = DaemonState::NotRunning;
        status.set_text(status_label(state));
        status.set_enabled(false);
        Some(state)
    }
}

/// Exact non-launching identity contract for the bare menubar artifact.
fn version_output(args: &[std::ffi::OsString]) -> Option<&'static str> {
    (args.len() == 1 && args[0] == "--version")
        .then_some(concat!("k2-menubar ", env!("CARGO_PKG_VERSION")))
}

fn main() {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(version) = version_output(&args) {
        println!("{version}");
        return;
    }

    #[cfg(target_os = "macos")]
    if let Err(err) = macos::run() {
        eprintln!("k2-menubar: {err}");
        std::process::exit(1);
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("k2-menubar is macOS-only");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::version_output;
    use std::ffi::OsString;

    #[test]
    fn version_is_exact_and_no_flag_keeps_the_launch_path() {
        assert_eq!(
            version_output(&[OsString::from("--version")]),
            Some("k2-menubar 0.41.3")
        );
        assert_eq!(version_output(&[]), None);
        assert_eq!(version_output(&[OsString::from("other")]), None);
    }
}
