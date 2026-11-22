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
// The source is chosen here. A timeline or a 3D scene the daemon serves
// (its core on: `real`, from live-doc-core) watches the file itself through
// the kit's keyed watch on K2's docs socket (TimelineLive, SceneLive), with
// no Sample data banner; the switch shows for it whether or not the preview
// key is on.
// Anything else plays its sample, with its banner, as before. Every view
// gets the file's path (LiveFileContext): the scene's picture is the file's
// own when the daemon draws it.

import { lazy, Suspense, useEffect, useMemo, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { previewTypeFor, useLivePreview, type LivePreviewType } from '@/lib/live-view/shared/preview-keys'
import { LiveEntrySwitchProvider, LiveEntrySwitchView, type LiveEntryMode, type LiveEntrySwitchValue } from './LiveEntrySwitch'
import { LiveFileContext } from './live-file'
import { PRODUCT_NAME } from '@/lib/brand'

export { WATCH_LIVE_LABEL } from './LiveEntrySwitch'

const LiveWorkbookSample = lazy(() => import('./LiveWorkbookSample'))
const LiveShellSample = lazy(() => import('./shell/LiveShellSample'))
const TimelineSample = lazy(() => import('./timeline/TimelineSample'))
const TimelineLive = lazy(() => import('./timeline/TimelineLive'))
const SceneLive = lazy(() => import('./scene/SceneLive'))

/** A real file's entry: the view on the file itself (the daemon serves it live): the timeline (T7) and the 3D scene (S8). */
const REAL: Partial<Record<LivePreviewType, { hint: string; view: (scope: ServerScope, filePath: string) => React.JSX.Element }>> = {
  timeline: { hint: 'Watches this file live: changes, playheads and who’s here.', view: (scope, filePath) => <TimelineLive scope={scope} filePath={filePath} /> },
  scene: { hint: 'Watches this file live: who’s here, what changes and where each one looks.', view: (scope, filePath) => <SceneLive scope={scope} filePath={filePath} /> },
}

const ENTRIES: Record<LivePreviewType, { group: string; hint: string; view: () => React.JSX.Element }> = {
  workbook: { group: 'Workbook view', hint: `Plays ${PRODUCT_NAME}’s sample workbook, not this file.`, view: () => <LiveWorkbookSample /> },
  scene: { group: 'Scene view', hint: `Plays ${PRODUCT_NAME}’s sample scene, not this file.`, view: () => <LiveShellSample kind="scene" /> },
  timeline: { group: 'Timeline view', hint: `Plays ${PRODUCT_NAME}’s sample edit, not this file.`, view: () => <TimelineSample /> },
}

export function LivePreviewEntry({
  filePath,
  children,
  onLiveChange,
  real,
}: {
  filePath: string
  children: React.ReactNode
  onLiveChange?: (live: boolean) => void
  /** The daemon serves this file live (live-doc-core says so): its server. Else the sample. */
  real?: ServerScope | null
}): React.JSX.Element {
  const type = previewTypeFor(filePath)
  const realEntry = type && real ? REAL[type] : undefined
  const on = useLivePreview(type) || !!realEntry
  const [mode, setMode] = useState<LiveEntryMode>('file')
  const live = on && type !== null && mode === 'live'
  useEffect(() => {
    onLiveChange?.(live)
  }, [live, onLiveChange])
  const file = useMemo(() => ({ path: filePath }), [filePath])
  const sw = useMemo<LiveEntrySwitchValue | null>(
    () => (type ? { group: ENTRIES[type].group, hint: realEntry?.hint ?? ENTRIES[type].hint, testIdPrefix: `live-${type}`, mode, onMode: setMode } : null),
    [type, mode, realEntry],
  )
  if (!on || !type || !sw) return <>{children}</>
  const entry = { hint: sw.hint, view: realEntry && real ? () => realEntry.view(real, filePath) : ENTRIES[type].view }
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-testid={`live-${type}-entry`} data-source={realEntry ? 'real' : 'sample'}>
      {mode === 'live' ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <LiveEntrySwitchProvider value={sw}>
            <LiveFileContext.Provider value={file}>
            <Suspense fallback={<div className="p-4 type-hint text-[var(--color-text-secondary)]">Loading the live view…</div>}>{entry.view()}</Suspense>
            </LiveFileContext.Provider>
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
