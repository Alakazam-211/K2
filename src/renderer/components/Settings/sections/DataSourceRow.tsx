// One source in Settings → Gardens → External Data (contract-sources-v1
// §8). Owner: B2.
//
// State dot, label, the one-word state, counts ("18 ok, 2 unknown": unknown
// is never "down"), age from the server's clock, triggers, the secret's
// name, badges (pinned, states, muted, TEST). The ⋯ menu sends one gesture
// each; the daemon decides and the section re-reads on `sources_changed`.

import React, { useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { SourceListRow } from '@/lib/sources/sources-types'
import {
  exportSource,
  muteSource,
  pauseSource,
  pinSource,
  refreshSource,
  removeSource,
  resumeSource,
  unmuteSource,
  unpinSource,
} from '@/lib/sources/sources-api'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { ageOf, btn, countsLine, dotColor, hhmm, muted, sourcesErrorText } from './data-sources-ui'

export type RowPanel = 'triggers' | 'simulate' | 'try' | null

export const REMOVE_TEXT = 'Gardens on other computers that show this will say removed. The name is held for 7 days.'

function Badge({ children, title }: { children: React.ReactNode; title?: string }): React.JSX.Element {
  return (
    <span title={title} className="px-1 text-[9px] uppercase tracking-wide border border-[var(--color-border)] text-[var(--color-text-secondary)]">
      {children}
    </span>
  )
}

export function stateText(row: SourceListRow): string {
  const v = row.view
  if (row.status === 'paused') return 'paused'
  if (v.display === 'pending') return 'waiting for the first check'
  if (v.display === 'stale') return `stale${v.reasonText ? ` · ${v.reasonText}` : ''}`
  if (v.display === 'paused') return `paused${v.reasonText ? ` · ${v.reasonText}` : ''}`
  if (v.soft.count > 0 && v.soft.level) return `${v.display} · soft ${v.soft.level} ${v.soft.count}/${v.soft.of}`
  if (v.soft.count > 0 && v.soft.count < v.soft.of) return `${v.display} · retrying ${v.soft.count}/${v.soft.of}`
  return v.display
}

export function DataSourceRow({
  scope,
  row,
  open,
  onOpen,
  onReview,
  onChanged,
  onError,
  onNote,
}: {
  scope: ServerScope
  row: SourceListRow
  open: RowPanel
  onOpen: (p: RowPanel) => void
  onReview: () => void
  onChanged: () => void
  onError: (text: string) => void
  onNote: (text: string) => void
}): React.JSX.Element {
  const [menu, setMenu] = useState(false)
  const v = row.view
  const age = ageOf(v)
  const proposed = row.status === 'proposed'
  const counts = countsLine(v.counts)

  const act = async (fn: () => Promise<unknown>, done?: string): Promise<void> => {
    setMenu(false)
    try {
      await fn()
      if (done) onNote(done)
      onChanged()
    } catch (e) {
      onError(sourcesErrorText(e))
    }
  }

  const items: Array<{ label: string; run: () => void; testid: string }> = proposed
    ? [{ label: 'Review', run: onReview, testid: 'review' }]
    : [
        { label: 'Try now', run: () => (setMenu(false), onOpen(open === 'try' ? null : 'try')), testid: 'try' },
        {
          label: 'Refresh',
          run: () =>
            void act(async () => {
              const r = await refreshSource(scope, row.name)
              onNote(r.queued ? `Checking ${row.label} now.` : `${row.label} was just checked.`)
            }),
          testid: 'refresh',
        },
        row.status === 'paused'
          ? { label: 'Resume', run: () => void act(() => resumeSource(scope, row.name)), testid: 'resume' }
          : { label: 'Pause', run: () => void act(() => pauseSource(scope, row.name)), testid: 'pause' },
        v.muted
          ? { label: 'Unmute', run: () => void act(() => unmuteSource(scope, row.name)), testid: 'unmute' }
          : {
              label: 'Mute 1 h',
              run: () => void act(() => muteSource(scope, row.name, 3600), `${row.label} is muted for an hour.`),
              testid: 'mute',
            },
        { label: 'Simulate…', run: () => (setMenu(false), onOpen('simulate')), testid: 'simulate' },
        row.pinned
          ? { label: 'Unpin', run: () => void act(() => unpinSource(scope, row.name)), testid: 'unpin' }
          : { label: 'Pin', run: () => void act(() => pinSource(scope, row.name)), testid: 'pin' },
        { label: 'Triggers…', run: () => (setMenu(false), onOpen(open === 'triggers' ? null : 'triggers')), testid: 'triggers' },
        {
          label: 'Export',
          run: () =>
            void act(async () => {
              const def = await exportSource(scope, row.name)
              await navigator.clipboard?.writeText(JSON.stringify(def, null, 2))
              onNote(`${row.label}'s definition is on the clipboard (secret names only).`)
            }),
          testid: 'export',
        },
        {
          label: 'Remove',
          run: () =>
            void (async () => {
              setMenu(false)
              const ok = await useConfirmDialogStore.getState().confirm({
                title: `Remove ${row.label}?`,
                message: REMOVE_TEXT,
                confirmLabel: 'Remove',
                destructive: true,
              })
              if (ok) await act(() => removeSource(scope, row.name), `${row.label} removed.`)
            })(),
          testid: 'remove',
        },
      ]

  return (
    <div className="relative" data-testid="sources-row" data-source={row.name}>
      <div className="flex items-center gap-2 py-1 text-[12px]">
        <span
          className="inline-block w-2 h-2 rounded-full shrink-0"
          style={{ background: dotColor(proposed ? 'proposed' : v.display), opacity: v.phase === 'stale' ? 0.5 : 1 }}
          aria-hidden
        />
        <span className="text-[var(--color-text-primary)] min-w-[9rem] truncate">{row.label}</span>
        <span className={`text-[11px] ${muted} truncate flex-1`}>
          {proposed ? (
            <>proposed{row.secret ? ` · wants secret ${row.secret}` : ''}</>
          ) : (
            <>
              {stateText(row)}
              {counts && ` · ${counts}`}
              {age && ` · ${age}`}
              {row.triggers > 0 && ` · ${row.triggers} trigger${row.triggers === 1 ? '' : 's'}`}
              {row.secret && ` · ${row.secret}`}
            </>
          )}
        </span>
        {row.pinned && <Badge title="Agents can't edit or remove it">pinned</Badge>}
        {row.keep === 'states' && <Badge title="No values are stored">states</Badge>}
        {v.muted && <Badge title={v.muted.reason ?? undefined}>muted until {hhmm(v.muted.until)}</Badge>}
        {v.simulated && <Badge title={`by ${v.simulated.by ?? 'someone'}`}>TEST {v.simulated.state}</Badge>}
        {proposed ? (
          <button type="button" className={btn} onClick={onReview} data-testid="sources-row-review">
            Review
          </button>
        ) : (
          <button type="button" className={btn} aria-label={`More for ${row.label}`} onClick={() => setMenu((m) => !m)} data-testid="sources-row-menu">
            ⋯
          </button>
        )}
      </div>
      {menu && (
        <div
          className="absolute right-0 top-7 z-[400] flex flex-col bg-[var(--color-bg-elevated)] border border-[var(--color-border)] min-w-[9rem]"
          role="menu"
          data-testid="sources-row-menu-list"
        >
          {items.map((it) => (
            <button
              key={it.testid}
              type="button"
              role="menuitem"
              className="text-left px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg)] cursor-pointer"
              onClick={it.run}
              data-testid={`sources-menu-${it.testid}`}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
