/// Desktop session name from the process environment. The Linux
/// stoplights use this to tell GNOME from a tiling compositor.
/// `XDG_CURRENT_DESKTOP` wins (`ubuntu:GNOME`, `Hyprland`). Empty off Linux.
#[tauri::command]
pub fn linux_desktop_session() -> String {
    #[cfg(target_os = "linux")]
    {
        std::env::var("XDG_CURRENT_DESKTOP")
            .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
            .unwrap_or_default()
    }
    #[cfg(not(target_os = "linux"))]
    String::new()
}
