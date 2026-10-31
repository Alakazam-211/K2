// The kit's live editor (`k2-mpc-client/editor`): a mountable CodeMirror 6 editor for one text
// root, in plain DOM. Every `@codemirror/*` package is a peer dependency: the host's single
// CodeMirror is used. See EDITOR.md.

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
export type { CloseEvent, CloseReason, LiveTransport, Rights, Welcome } from "../wire";
export { ByteIndex } from "./bytes";
export { readableOn, contrast, luminance } from "./colour";
export { CSS_VARS } from "./theme";
