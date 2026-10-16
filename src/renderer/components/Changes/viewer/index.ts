// The Changes tab's diff viewer (C3). C2's `ChangesDiffSlot` renders
// `<ChangesDiffViewer {...props} />`. Nothing imported here pulls in
// CodeMirror: the viewer itself loads lazily.
export { ChangesDiffViewer } from './ChangesDiffViewer'
export { diffRowKey, expectedDiffMode } from './types'
export type {
  ChangesDiffViewerProps,
  ChangesDiffFile,
  ChangesDiffGroup,
  ChangesDiffLayout,
  DiffSide,
  DiffSidesResponse,
  DiffSidesState,
} from './types'
