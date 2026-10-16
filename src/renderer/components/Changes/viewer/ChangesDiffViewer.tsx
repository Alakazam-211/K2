// What C2's `ChangesDiffSlot` renders. A thin lazy wrapper: the viewer and
// @codemirror/merge load as their own chunk the first time a Changes tab
// shows a diff, not with the app shell.

import { lazy, Suspense } from 'react'
import type { ChangesDiffViewerProps } from './types'

const ChangesDiffView = lazy(() => import('./ChangesDiffView').then((m) => ({ default: m.ChangesDiffView })))

export function ChangesDiffViewer(props: ChangesDiffViewerProps): React.JSX.Element {
  return (
    <Suspense
      fallback={
        <div className="h-full flex items-center justify-center text-xs text-[var(--color-text-muted)]" data-testid="diff-viewer-loading">
          Loading diff viewer…
        </div>
      }
    >
      <ChangesDiffView {...props} />
    </Suspense>
  )
}
