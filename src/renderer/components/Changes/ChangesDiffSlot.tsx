// C2/C3 seam — the Changes tab's diff area. C2 mounts this with
// `ChangesDiffSlotProps` (`./diff-slot-types.ts`); the body is C3's
// @codemirror/merge viewer (`./viewer/*`), which loads as a lazy chunk.

import { ChangesDiffViewer } from './viewer'
import type { ChangesDiffSlotProps } from './diff-slot-types'

export type { ChangesDiffSlotProps } from './diff-slot-types'

export function ChangesDiffSlot(props: ChangesDiffSlotProps): React.JSX.Element {
  return (
    <div data-changes-diff-slot="" data-mode={props.mode} data-view={props.view} className="h-full w-full min-h-0">
      <ChangesDiffViewer {...props} />
    </div>
  )
}
