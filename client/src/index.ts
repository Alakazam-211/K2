// The kit's JS client for the hunk channel (K1): host-agnostic, plain TypeScript.
// See WIRE.md for the frames a host carries.

export { LiveText, diffText, frameBytes } from "./live";
export type { ChangeInfo, DuplicateRisk, Landed, LiveOptions, OpId, Presence, ReapplyGuard, Rejection, RemoteCaret, UndoResult } from "./live";
export { codemirrorLive, changesToHunks, hunksToChanges, remoteChange } from "./codemirror";
export type { CodemirrorLiveOptions } from "./codemirror";
export { transform, Bias } from "./position";
export { Carried, rebaseOurs, rebaseIncoming, mapPoint, mapRange } from "./rebase";
export type { LiveConflict, RebaseResult } from "./rebase";
export { apply, validate, coalesce, compose, invert, utf8Len, byteToIndex, indexToByte } from "./hunk";
export type { Hunk, HunkError } from "./hunk";
export { ERR, FRAME_MARGIN, FRAME_MAX, canRetry } from "./wire";
export type {
  CaretAt,
  ChangeEvent,
  ClientFrame,
  CloseEvent,
  CloseReason,
  ErrorReport,
  Handle,
  LineSpan,
  ConnectOptions,
  LiveTransport,
  ParticipantInfo,
  Rights,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
  Welcome,
} from "./wire";
