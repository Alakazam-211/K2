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

/** The Remove Workspace dialog (RemoveWorkspaceDialog): opened from the
 *  sidebar and from Settings → Workspaces (0.46.7), so above Settings and
 *  below toasts. */
export const REMOVE_WORKSPACE_Z = 1000

/** Add Workspace and Initialize git (AddWorkspaceDialog, GitInitDialog):
 *  opened after the folder picker (HOST_PICKER_Z) from Settings → Workspaces,
 *  Home (⌘O) and the sidebar, so above Settings (200) and Home (150). At 50
 *  they opened hidden under Settings/Home and a new folder seemed to do
 *  nothing (Rosson 2026-11-04). */
export const ADD_WORKSPACE_Z = 1000

/** The floating call tile (Voice/CallTile): over the room and over Zen
 *  (150), so a video call stays in view on every page; below Settings (200),
 *  where the top bar's call controls take over, and below every modal, menu
 *  and toast. It steps aside while a menu or popover overlaps it. */
export const CALL_TILE_Z = 160

/** The call's video full screen (Voice/CallFullscreen): over everything in
 *  the window, Settings and modals included, since it is the whole screen
 *  while it's on; below toasts (9999), so a call notice still shows. */
export const CALL_FULLSCREEN_Z = 9000
