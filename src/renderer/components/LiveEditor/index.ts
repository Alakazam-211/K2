// K2's Live editor. K2 owns this code: the editor UI (the CodeMirror binding, the
// mount, carets and labels, notes, theme) moved here from the kit (Rosson 2026-10-31;
// K2 Cores agreed), and this is now its only copy. The library it calls (LiveText, hunks,
// rebase, the wire) stays vendored from k2-mpc-base (`vendor/k2-mpc-base/client`), imported
// through the `@k2/mpc-client` alias. Tests: ./test (the kit's editor tests, moved with it).
//
// What it is: a mountable CodeMirror 6 editor for one text root, in plain DOM, on the app's
// own CodeMirror. Behaviour is the kit's as of the move (its EDITOR.md describes it).

export { mountLiveEditor, DEFAULT_STRINGS, HISTORY_ERROR, CLOSE_RETRY_ERROR, LiveEditorError } from "./mount";
export type {
  BannerInfo,
  ConnectionState,
  EditorRejection,
  EditorStrings,
  LabelMode,
  LiveEditorEvents,
  LiveEditorHandle,
  LiveEditorOptions,
  ReapplyResult,
  RejectionActions,
  Timers,
} from "./mount";
export type { ClaimEvent, RemovedTextNote } from "./notes";
export type { CloseEvent, CloseReason, LiveTransport, Rights, Welcome } from "@k2/mpc-client";
export { ByteIndex } from "./bytes";
export { readableOn, contrast, luminance } from "./colour";
export { CSS_VARS } from "./theme";
