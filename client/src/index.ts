// The kit's JS client for the hunk channel: host-agnostic, plain TypeScript, no DOM. The
// host draws its own editor from it (HOST.md); WIRE.md has the frames a host carries.

export { LiveText, diffText, frameBytes, setDefaultLiveTimers } from "./live";
export type { ChangeInfo, DuplicateRisk, Landed, LiveOptions, LiveTimers, OpId, Placement, Presence, ReapplyGuard, Rejection, RemoteCaret, UndoResult } from "./live";
export {
  CLAIMS_FEATURE,
  CLAIM_KEYS_MAX,
  CLAIM_NOTE_MAX,
  ClaimRejected,
  ELEM_SELECTION_MAX,
  FOCUS_FEATURE,
  FRAME_MS,
  PRESENCE_ADDRESS_MAX,
  PRESENCE_FEATURE,
  PresenceChannel,
  RANGE_FEATURE,
  TIME_POSITION_FEATURE,
  VIEW_ADDRESS_FEATURE,
  claimEndReason,
  defaultScheduler,
  participantKind,
  rangeToUtf16,
} from "./presence";
export type { ClaimEnded, ClaimEndReason, ClaimOptions, ClaimRejectedReason, ClaimView, FrameScheduler, InsertHereResult, InsertPreview, InsertRefusal, Mark, MarkPlacement, Participant, ParticipantKind, PresenceChannelOptions, PresenceFrameType, PresencePeer, PresenceRefusal, Range16 } from "./presence";
export { transform, Bias } from "./position";
export { KEYED_WATCH_FEATURE, KeyedWatch, REPORT_EVERY, keyedStateJson, mapKey } from "./keyed";
export type { KeyedRootState, KeyedState, KeyedWatchOptions, ResyncReason } from "./keyed";
export { CONTAINER_DEPTH_MAX, KeyedView, VIEW_TEXT_MAX } from "./view";
export { mul, placementMatrix, stepMatrix, worldMatrix } from "./placement";
export type { Mat4, PlacementLookup, Precision } from "./placement";
export {
  CELLS_FEATURE,
  CELLS_RANGES_MAX,
  CELL_CLAIMS_FEATURE,
  blankNumbers,
  cellsA1,
  colLabel,
  colLetters,
  colNumber,
  colOfCellField,
  cornerA1,
  cornerAt,
  cornerNumbers,
  cornerProblem,
  fieldClass,
  gridNumbers,
  rangeA1,
} from "./grid";
export type { CornerProblem, GridLine, GridNumbers } from "./grid";
export { MAX_CLIENT_ID, MAX_KEY_LEN, MAX_KIT_CLIENT, MAX_PREFIX_LEN, MAX_TAG_LEN, formatKey, markKey, nextFor, parseKey, parseTag, retiredMark, writerTag } from "./mint";
export type { MintError, TaggedKey } from "./mint";
export { resolveParents, resolveRef, resolveRefs } from "./refs";
export type { Lists, Ref, RefState } from "./refs";
export { Carried, rebaseOurs, rebaseIncoming, mapPoint, mapRange } from "./rebase";
export type { LiveConflict, RebaseResult } from "./rebase";
export { apply, validate, coalesce, compose, invert, utf8Len, byteToIndex, indexToByte } from "./hunk";
export type { Hunk, HunkError } from "./hunk";
export { ERR, FRAME_MARGIN, FRAME_MAX, WIRE_VERSION, canRetry } from "./wire";
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
  ClaimActor,
  Device,
  ElemClaim,
  ElemFocus,
  ElemRange,
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
  KeyedPageFrame,
  KeyedRootKind,
  KeyedSchema,
  KeyedSnapshotFrame,
  KeyedUnitsPage,
  KeyedValue,
  OpsChangeFrame,
  OutsideSaveFrame,
  TimePositionJson,
  TimePositionSet,
  VerbRefusedFrame,
  WatchActor,
  WatchOp,
  ContainerDelta,
  UnitDelta,
  ViewContainer,
  NestingJson,
  ViewSchemaJson,
  ViewSnapshotJson,
  ViewSrc,
  ViewUnit,
  PlacementJson,
  RotationJson,
  SpaceJson,
  SpaceSlotJson,
  StepJson,
  Band,
  CellClaim,
  CellRange,
  Cells,
  ContainerDerived,
  Corner,
  FieldClass,
  FrameDerived,
  GridNumbering,
  GridSlot,
  StyleHint,
  UnitDerived,
  Written,
} from "./wire";
