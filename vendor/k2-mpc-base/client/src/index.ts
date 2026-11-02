// The kit's JS client for the hunk channel (K1): host-agnostic, plain TypeScript, no DOM. The
// host draws its own editor from it (HOST.md); WIRE.md has the frames a host carries.

export { LiveText, diffText, frameBytes, setDefaultLiveTimers } from "./live";
export type { ChangeInfo, DuplicateRisk, Landed, LiveOptions, LiveTimers, OpId, Placement, Presence, ReapplyGuard, Rejection, RemoteCaret, UndoResult } from "./live";
export { FRAME_MS, defaultScheduler, participantKind } from "./presence";
export type { FrameScheduler, InsertHereResult, InsertPreview, InsertRefusal, Mark, MarkPlacement, Participant, ParticipantKind, Range16 } from "./presence";
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
  Device,
  LiveTransport,
  NextStep,
  ParticipantInfo,
  Rights,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
  Welcome,
} from "./wire";
