// prd-zen-user-widgets-v2 S3 — the sealed frame of one custom widget.
// Placeholder until the frame host lands (the next S3 commit): it draws a
// quiet box and never runs widget code.

import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetErrorCode } from '@/lib/zen/zen-custom-grants'

/** Why the bundle couldn't be shown (the parent draws K2's card). */
export interface ZenCustomLoadProblem {
  code: ZenWidgetErrorCode
  message: string
}

export function ZenCustomFrame({
  widget,
}: {
  widget: ZenCustomWidgetPayload
  gardenId: string
  bridge: ZenWidgetBridge
  onProblem(problem: ZenCustomLoadProblem): void
}): React.JSX.Element {
  return <div data-zen-custom-frame-slot={widget.id} className="flex-1" />
}
