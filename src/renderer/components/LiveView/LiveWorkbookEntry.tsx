// The way in, behind the owner's `liveWorkbookPreview` key: an .xlsx file tab
// gets a "File | Watch live (preview)" switch above its content. "Watch live"
// mounts the live workbook view with the sample source (sample data, never
// this file: its banner says so). With the key off, or for any other file,
// this renders its children and nothing else.
//
// The source is chosen here. Today it is always the sample; the real one is
// KeyedWatchWorkbookSource in lib/live-view/source.ts (the kit's keyed
// watch on a doc socket), not wired to a socket yet.

import { lazy, Suspense, useState } from 'react'
import { Segmented } from '@/components/ui'
import { isWorkbookPath, useLiveWorkbookPreview } from '@/lib/live-view/preview-key'

const LiveWorkbookSample = lazy(() => import('./LiveWorkbookSample'))

export const WATCH_LIVE_LABEL = 'Watch live (preview)'

export function LiveWorkbookEntry({ filePath, children }: { filePath: string; children: React.ReactNode }): React.JSX.Element {
  const on = useLiveWorkbookPreview()
  const [mode, setMode] = useState<'file' | 'live'>('file')
  if (!on || !isWorkbookPath(filePath)) return <>{children}</>
  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-hidden" data-testid="live-workbook-entry">
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] bg-[var(--color-bg-stripe)] px-3 py-1">
        <Segmented<'file' | 'live'>
          aria-label="Workbook view"
          value={mode}
          onChange={setMode}
          options={[
            { value: 'file', label: 'File' },
            { value: 'live', label: WATCH_LIVE_LABEL, buttonProps: { 'data-testid': 'live-workbook-watch' } },
          ]}
        />
        {mode === 'live' ? <span className="type-hint text-[var(--color-text-muted)]">Plays K2’s sample workbook, not this file.</span> : null}
      </div>
      {mode === 'live' ? (
        <div className="min-h-0 flex-1 overflow-hidden">
          <Suspense fallback={<div className="p-4 type-hint text-[var(--color-text-muted)]">Loading the live view…</div>}>
            <LiveWorkbookSample />
          </Suspense>
        </div>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col overflow-hidden">{children}</div>
      )}
    </div>
  )
}
