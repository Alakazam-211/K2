// The way into the live views, one registry for every type, each behind its
// own preview switch (lib/live-view/shared/preview-keys.ts): a workbook,
// 3D scene or timeline file tab gets a "File | Watch live (preview)" switch
// above its content. "Watch live" mounts that type's view with its sample
// (sample data, never this file: the banner says so) and hides the tab's
// any-file panel (`onLiveChange`), so the tab never has two side panels.
// With the type's switch off, or for any other file, this renders its
// children and nothing else.
//
// The source is chosen here. Today it is always the sample; the real ones
// (the kit's keyed watch on a doc socket) come in each view's later phases.

import { lazy, Suspense, useEffect, useState } from 'react'
import { Segmented } from '@/components/ui'
import { previewTypeFor, useLivePreview, type LivePreviewType } from '@/lib/live-view/shared/preview-keys'

const LiveWorkbookSample = lazy(() => import('./LiveWorkbookSample'))
const LiveShellSample = lazy(() => import('./shell/LiveShellSample'))
const TimelineSample = lazy(() => import('./timeline/TimelineSample'))

export const WATCH_LIVE_LABEL = 'Watch live (preview)'

const ENTRIES: Record<LivePreviewType, { group: string; hint: string; view: () => React.JSX.Element }> = {
  workbook: { group: 'Workbook view', hint: 'Plays K2’s sample workbook, not this file.', view: () => <LiveWorkbookSample /> },
  scene: { group: 'Scene view', hint: 'Plays K2’s sample scene, not this file.', view: () => <LiveShellSample kind="scene" /> },
  timeline: { group: 'Timeline view', hint: 'Plays K2’s sample edit, not this file.', view: () => <TimelineSample /> },
}

export function LivePreviewEntry({ filePath, children, onLiveChange }: { filePath: string; children: React.ReactNode; onLiveChange?: (live: boolean) => void }): React.JSX.Element {
  const type = previewTypeFor(filePath)
  const on = useLivePreview(type)
  const [mode, setMode] = useState<'file' | 'live'>('file')
  const live = on && type !== null && mode === 'live'
  useEffect(() => {
    onLiveChange?.(live)
  }, [live, onLiveChange])
  if (!on || !type) return <>{children}</>
  const entry = ENTRIES[type]
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-testid={`live-${type}-entry`}>
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1">
        <Segmented<'file' | 'live'>
          aria-label={entry.group}
          value={mode}
          onChange={setMode}
          options={[
            { value: 'file', label: 'File' },
            { value: 'live', label: WATCH_LIVE_LABEL, buttonProps: { 'data-testid': `live-${type}-watch` } },
          ]}
        />
        {mode === 'live' ? <span className="type-hint text-[var(--color-text-secondary)]">{entry.hint}</span> : null}
      </div>
      {mode === 'live' ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <Suspense fallback={<div className="p-4 type-hint text-[var(--color-text-secondary)]">Loading the live view…</div>}>{entry.view()}</Suspense>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">{children}</div>
      )}
    </div>
  )
}
