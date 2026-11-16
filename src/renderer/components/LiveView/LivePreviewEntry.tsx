// The way into the live views, one registry for every type, each behind its
// own preview switch (lib/live-view/shared/preview-keys.ts): a workbook,
// 3D scene or timeline file tab gets a "File | Watch live (preview)" switch.
// Over the file it is a row above the content; while watching, the live
// view's header is the tab's only header, so the switch moves into it
// (LiveEntrySwitch) and the tab hides its own toolbar and any-file panel
// (`onLiveChange`): the file is named once and there is one side panel.
// "Watch live" mounts that type's view with its sample (sample data, never
// this file: the banner says so). With the type's switch off, or for any
// other file, this renders its children and nothing else.
//
// The source is chosen here. Today it is always the sample; the real ones
// (the kit's keyed watch on a doc socket) come in each view's later phases.

import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import { previewTypeFor, useLivePreview, type LivePreviewType } from '@/lib/live-view/shared/preview-keys'
import { LiveEntrySwitchProvider, LiveEntrySwitchView, type LiveEntryMode, type LiveEntrySwitchValue } from './LiveEntrySwitch'

export { WATCH_LIVE_LABEL } from './LiveEntrySwitch'

const LiveWorkbookSample = lazy(() => import('./LiveWorkbookSample'))
const LiveShellSample = lazy(() => import('./shell/LiveShellSample'))
const TimelineSample = lazy(() => import('./timeline/TimelineSample'))

const ENTRIES: Record<LivePreviewType, { group: string; hint: string; view: () => React.JSX.Element }> = {
  workbook: { group: 'Workbook view', hint: 'Plays K2’s sample workbook, not this file.', view: () => <LiveWorkbookSample /> },
  scene: { group: 'Scene view', hint: 'Plays K2’s sample scene, not this file.', view: () => <LiveShellSample kind="scene" /> },
  timeline: { group: 'Timeline view', hint: 'Plays K2’s sample edit, not this file.', view: () => <TimelineSample /> },
}

export function LivePreviewEntry({ filePath, children, onLiveChange }: { filePath: string; children: React.ReactNode; onLiveChange?: (live: boolean) => void }): React.JSX.Element {
  const type = previewTypeFor(filePath)
  const on = useLivePreview(type)
  const [mode, setMode] = useState<LiveEntryMode>('file')
  const live = on && type !== null && mode === 'live'
  useEffect(() => {
    onLiveChange?.(live)
  }, [live, onLiveChange])
  const sw = useMemo<LiveEntrySwitchValue | null>(
    () => (type ? { group: ENTRIES[type].group, hint: ENTRIES[type].hint, testIdPrefix: `live-${type}`, mode, onMode: setMode } : null),
    [type, mode],
  )
  if (!on || !type || !sw) return <>{children}</>
  const entry = ENTRIES[type]
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-testid={`live-${type}-entry`}>
      {mode === 'live' ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <LiveEntrySwitchProvider value={sw}>
            <Suspense fallback={<div className="p-4 type-hint text-[var(--color-text-secondary)]">Loading the live view…</div>}>{entry.view()}</Suspense>
          </LiveEntrySwitchProvider>
        </div>
      ) : (
        <>
          <div data-testid={`live-${type}-switch-row`} className="flex flex-none items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1">
            <LiveEntrySwitchView v={sw} />
            <span className="type-hint text-[var(--color-text-secondary)]">{entry.hint}</span>
          </div>
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden">{children}</div>
        </>
      )}
    </div>
  )
}
