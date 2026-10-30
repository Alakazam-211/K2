// The kit's JS client for the hunk channel (K1): host-agnostic, plain TypeScript.
// See WIRE.md for the frames a host carries.

export { LiveText, diffText } from "./live";
export type { ChangeInfo, LiveOptions, OpId, Rejection, RemoteCaret } from "./live";
export { codemirrorLive, changesToHunks, hunksToChanges, remoteChange } from "./codemirror";
export type { CodemirrorLiveOptions } from "./codemirror";
export { transform, Bias } from "./position";
export { Carried, rebaseOurs, rebaseIncoming, mapPoint, mapRange } from "./rebase";
export type { LiveConflict, RebaseResult } from "./rebase";
export { apply, validate, coalesce, compose, invert, utf8Len, byteToIndex, indexToByte } from "./hunk";
export type { Hunk, HunkError } from "./hunk";
export { ERR } from "./wire";
export type {
  CaretAt,
  ChangeEvent,
  ClientFrame,
  ErrorReport,
  Handle,
  ParticipantInfo,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
} from "./wire";
