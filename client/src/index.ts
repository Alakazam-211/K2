// The kit's JS client for the hunk channel (K1): host-agnostic, plain TypeScript, no DOM. The
// host draws its own editor from it (HOST.md); WIRE.md has the frames a host carries.

export { LiveText, diffText, frameBytes, setDefaultLiveTimers } from "./live";
export type { ChangeInfo, DuplicateRisk, Landed, LiveOptions, LiveTimers, OpId, Placement, Presence, ReapplyGuard, Rejection, RemoteCaret, UndoResult } from "./live";
export { CLAIMS_FEATURE, CLAIM_KEYS_MAX, CLAIM_NOTE_MAX, ClaimRejected, ELEM_SELECTION_MAX, FRAME_MS, PRESENCE_FEATURE, PresenceChannel, defaultScheduler, participantKind } from "./presence";
export type { ClaimOptions, ClaimRejectedReason, FrameScheduler, InsertHereResult, InsertPreview, InsertRefusal, Mark, MarkPlacement, Participant, ParticipantKind, PresenceChannelOptions, PresenceFrameType, PresencePeer, PresenceRefusal, Range16 } from "./presence";
export { transform, Bias } from "./position";
export { MAX_CLIENT_ID, MAX_KEY_LEN, MAX_KIT_CLIENT, MAX_PREFIX_LEN, MAX_TAG_LEN, formatKey, markKey, nextFor, parseKey, parseTag, retiredMark, writerTag } from "./mint";
export type { MintError, TaggedKey } from "./mint";
export { resolveParents, resolveRef, resolveRefs } from "./refs";
export type { Lists, Ref, RefState } from "./refs";
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
  ElemSelection,
  LiveTransport,
  NextStep,
  ParticipantInfo,
  Point,
  Rect,
  Rights,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
  Welcome,
} from "./wire";
