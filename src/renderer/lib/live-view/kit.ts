// The kit adapter for the live workbook view: the ONLY file in lib/live-view
// (and components/LiveView) that names the kit's client. Everything here is
// the vendored kit's own (`vendor/k2-mpc-base/client`, imported as
// `@k2/mpc-client`, v0.16): the keyed watch's frames and its watcher
// (`KeyedWatch`, which keeps the state), the presence and claim roster
// events, the welcome, `mapKey` and `VIEW_TEXT_MAX`.
//
// Nothing is mirrored or forked here. A field the kit doesn't have belongs
// in `xlsx-provisional.ts`; the view's immutable fold over the kit's watcher
// is `keyed-fold.ts`.

export type {
  ClaimActor,
  ClientFrame,
  ElemFocus,
  ElemSelection,
  ErrorReport,
  Handle,
  KeyedPageFrame,
  KeyedRootState,
  KeyedSchema,
  KeyedSnapshotFrame,
  KeyedState,
  KeyedUnitsPage,
  KeyedValue,
  LineSpan,
  OpsChangeFrame,
  Point,
  Rect,
  ResyncReason,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
  ViewContainer,
  ViewSchemaJson,
  ViewSnapshotJson,
  ViewSrc,
  ViewUnit,
  WatchActor,
  WatchOp,
  Welcome,
} from '@k2/mpc-client'

export { KEYED_WATCH_FEATURE, KeyedWatch, REPORT_EVERY, VIEW_TEXT_MAX, keyedStateJson, mapKey } from '@k2/mpc-client'
