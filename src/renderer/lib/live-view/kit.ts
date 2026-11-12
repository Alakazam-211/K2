// The kit adapter for the live workbook view: the ONLY file in lib/live-view
// (and components/LiveView) that names the kit's client. Everything here is
// the vendored kit's own (`vendor/k2-mpc-base/client`, imported as
// `@k2/mpc-client`, v0.17): the keyed watch's frames and its watcher
// (`KeyedWatch`, which keeps the state), the presence and claim roster
// events, the welcome, `mapKey` and `VIEW_TEXT_MAX`; since v0.17 also the
// live grid's derived values and corners (`FrameDerived`, `UnitDerived`,
// `ContainerDerived`, `StyleHint`, `Corner`, `CellRange`, `Written`, `Band`).
//
// Nothing is mirrored or forked here. A field the kit doesn't have belongs
// in `xlsx-provisional.ts`; the view's immutable fold over the kit's watcher
// is `keyed-fold.ts`.

export type {
  Band,
  CellRange,
  ClaimActor,
  ClientFrame,
  ContainerDerived,
  Corner,
  ElemFocus,
  ElemSelection,
  ErrorReport,
  FrameDerived,
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
  StyleHint,
  Transport,
  ViewContainer,
  ViewSchemaJson,
  ViewSnapshotJson,
  ViewSrc,
  UnitDerived,
  ViewUnit,
  WatchActor,
  WatchOp,
  Welcome,
  Written,
} from '@k2/mpc-client'

export { KEYED_WATCH_FEATURE, KeyedWatch, REPORT_EVERY, VIEW_TEXT_MAX, keyedStateJson, mapKey } from '@k2/mpc-client'
