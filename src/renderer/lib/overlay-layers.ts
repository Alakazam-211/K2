// Shared z-index layers for full-window overlays that must stack in a
// known order.
//
// The Settings overlay is a full-viewport, opaque layer on top of the
// (still-mounted) workspace shell. Any app-level modal that Settings can
// open — e.g. the remote host file picker behind Settings → Agent →
// Identity → Upload when connected to a server — must sit ABOVE it, or it
// opens hidden behind Settings and only shows once you leave Settings
// ("opens on a different page").

/** The Settings full-viewport overlay (App.tsx). */
export const SETTINGS_OVERLAY_Z = 200

/** The remote host file/folder picker (RemoteFolderPicker). Opened from
 *  Settings, the Projects page and the workspace shell, so it sits above
 *  all of them — same modal layer as IconCropDialog, which follows it in
 *  the icon flow — and below toasts (9999). */
export const HOST_PICKER_Z = 1000
