// prd-zen-garden-sync-defaults-v1 GS33–GS34c — Settings → Gardens: two
// switches per Garden row ("Sync with K2's default" and "Sync theme with
// K2's Basic"), the turn-on confirm with Preview in Zen, Undo, Keep my
// previous look, and the unseen Garden news list.
//
// Thin client over `lib/zen/zen-sync.ts`: the daemon owns each Garden's
// state; this draws it and posts gestures. Nothing pops up (GS34c): changes
// show here, in the What's new side card and in the CLI. A daemon without
// `zen-sync-v1` draws nothing.

import React, { useEffect, useState } from 'react'
import { Toggle } from '@/components/ui/Toggle'
import { useZenSettingsStore } from '@/lib/zen/zen-settings'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import {
  keepZenPreviousLook,
  loadZenSync,
  markZenNewsSeen,
  setZenGardenSync,
  undoZenGardenSync,
  useZenSyncPreviewStore,
  useZenSyncStore,
  zenNewsCardText,
  type ZenNewsItem,
  type ZenSyncPart,
  type ZenSyncPartName,
  type ZenSyncRow,
} from '@/lib/zen/zen-sync'

const LINK =
  'text-[11px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'

// Settings → Gardens already re-reads its list on every local `zen_changed`
// (`watchLocalZenChanged` in ZenGardensSection). Rather than hold a second
// socket, the sync state re-reads whenever that list does, so a toggle here,
// in Zen or from the CLI shows in step.
let users = 0
let unsubscribe: (() => void) | null = null

/** Keep the sync store live while any sync control is mounted. */
export function useZenSyncLive(): void {
  useEffect(() => {
    users += 1
    if (users === 1) {
      void loadZenSync()
      unsubscribe = useZenSettingsStore.subscribe((s, prev) => {
        if (s.gardens !== prev.gardens || s.setUp !== prev.setUp) void loadZenSync()
      })
    }
    return () => {
      users -= 1
      if (users === 0 && unsubscribe) {
        unsubscribe()
        unsubscribe = null
      }
    }
  }, [])
}

/** The switch's label (GS33). */
export function zenSyncSwitchLabel(row: ZenSyncRow, part: ZenSyncPartName): string {
  if (part === 'page') return 'Sync with K2’s default'
  if (row.themeBuiltin) return `Sync theme with K2’s ${row.themeBase.label}`
  return `Sync theme with K2’s ${row.themeBase.label} under your ${row.themeLabel} theme`
}

/** The quiet lines under a switch (GS33). */
export function zenSyncCaptions(row: ZenSyncRow, part: ZenSyncPartName): string[] {
  const st: ZenSyncPart = row[part]
  const what = part === 'page' ? 'layout and look' : 'theme'
  if (st.mode === 'synced') {
    const lines = [`Gets K2’s improvements to this Garden’s ${what}.`]
    if (part === 'page' && row.ownChanges && row.ownChanges.length > 0) {
      lines.push('Your own changes sit on top of K2’s default. Turn sync off to keep this Garden exactly as it is now.')
    }
    return lines
  }
  const lines = ['Your own copy. K2 updates never change it.']
  if (part === 'theme' && st.k2Version) lines.push(`K2 themes from ${st.k2Version}.`)
  return lines
}

/** Open this Garden in Zen previewing K2's default for `part` (GS34). */
export function startZenSyncPreview(gardenId: string, part: ZenSyncPartName): void {
  const win = useZenWindowStore.getState()
  win.setGarden(gardenId)
  useZenSyncPreviewStore.getState().start(gardenId, part)
  win.setOn(true)
  useSettingsStore.getState().closeSettings()
}

function PartControl({
  row,
  part,
  busy,
  run,
}: {
  row: ZenSyncRow
  part: ZenSyncPartName
  busy: boolean
  run(what: () => Promise<void>, done?: string): void
}): React.JSX.Element {
  const [confirm, setConfirm] = useState(false)
  const st = row[part]
  const on = st.mode === 'synced'
  const label = zenSyncSwitchLabel(row, part)
  const turnOn = (): void => {
    setConfirm(false)
    run(() => setZenGardenSync(row.id, part, true), `${row.name} now syncs with K2’s default`)
  }
  return (
    <div data-zen-sync-part={part} data-zen-sync-mode={st.mode} className="flex items-start gap-2 py-1">
      <span data-testid={`zen-sync-${part}-${row.id}`} className="contents">
        <Toggle
          checked={on}
          aria-label={label}
          disabled={busy}
          onChange={(next) => {
            if (!next) run(() => setZenGardenSync(row.id, part, false), `${row.name} is now your own copy`)
            else setConfirm(true)
          }}
        />
      </span>
      <div className="min-w-0 flex-1">
        <div className="text-xs text-[var(--color-text-primary)]">{label}</div>
        {zenSyncCaptions(row, part).map((c) => (
          <div key={c} className="text-[10px] text-[var(--color-text-muted)]">
            {c}
          </div>
        ))}
        {!on && st.newerDefault && (
          <div className="text-[10px] text-[var(--color-text-muted)]" data-zen-sync-newer={part}>
            K2’s default has changed since your copy ·{' '}
            <button type="button" className={LINK} onClick={() => startZenSyncPreview(row.id, part)}>
              Preview
            </button>
          </div>
        )}
        {confirm && (
          <div className="mt-1 flex flex-wrap items-center gap-2" data-zen-sync-confirm={part}>
            <span className="text-[11px] text-[var(--color-text-secondary)]">
              {part === 'page' ? 'This Garden can look different with K2’s default.' : 'This Garden’s theme can look different.'}
            </span>
            <button type="button" className={BTN} disabled={busy} onClick={() => startZenSyncPreview(row.id, part)}>
              Preview in Zen
            </button>
            <button type="button" className={BTN} disabled={busy} onClick={turnOn} data-zen-sync-turn-on={part}>
              Turn on
            </button>
            <button type="button" className={BTN} disabled={busy} onClick={() => setConfirm(false)}>
              Cancel
            </button>
          </div>
        )}
      </div>
    </div>
  )
}

/** The two switches for one Garden row (GS33). Nothing when this daemon
 *  has no Garden sync, or before its first answer. */
export function ZenGardenSyncControls({ garden }: { garden: { id: string; name: string } }): React.JSX.Element | null {
  useZenSyncLive()
  const row = useZenSyncStore((s) => s.list?.gardens.find((g) => g.id === garden.id) ?? null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  if (!row) return null

  const run = (what: () => Promise<void>, done?: string): void => {
    setBusy(true)
    setError(null)
    what()
      .then(() => {
        if (done) {
          useToastStore.getState().addToast(done, 'success', 8000, {
            label: 'Undo',
            onClick: () => void undoZenGardenSync(garden.id).catch((e: unknown) => setError(String(e))),
          })
        }
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false))
  }

  const keep = row.keepPrevious.page || row.keepPrevious.theme
  const keepPart = row.keepPrevious.page && row.keepPrevious.theme ? 'both' : row.keepPrevious.page ? 'page' : 'theme'
  return (
    <div className="mt-1 ml-12" data-zen-sync-controls={garden.id}>
      <PartControl row={row} part="page" busy={busy} run={run} />
      <PartControl row={row} part="theme" busy={busy} run={run} />
      {keep && (
        <div className="text-[10px] text-[var(--color-text-muted)]" data-zen-sync-keep-previous="">
          This release changed this Garden’s look ·{' '}
          <button
            type="button"
            className={LINK}
            disabled={busy}
            onClick={() => run(() => keepZenPreviousLook(garden.id, keepPart), `${row.name} keeps its previous look`)}
          >
            Keep my previous look
          </button>
        </div>
      )}
      {row.undo && (
        <button
          type="button"
          className={LINK}
          disabled={busy}
          data-zen-sync-undo=""
          onClick={() => run(() => undoZenGardenSync(garden.id))}
        >
          Undo
        </button>
      )}
      {error && (
        <p role="alert" className="text-[11px] text-[var(--color-status-error)] mt-1">
          {error}
        </p>
      )}
    </div>
  )
}

function newsLine(item: ZenNewsItem): string {
  const t = zenNewsCardText(item)
  return [t.title, t.lead, t.body].filter(Boolean).join(' · ')
}

/** Settings → Gardens: unseen Garden news (GS42 — what a narrow window's
 *  card didn't show stays here). Leaving the section marks what it showed
 *  as seen. */
export function ZenGardenSettingsNews(): React.JSX.Element | null {
  useZenSyncLive()
  const news = useZenSyncStore((s) => s.news)
  const items = news?.items ?? []
  const ids = items.map((i) => i.id).join('\n')
  useEffect(() => {
    if (ids === '') return
    return () => {
      void markZenNewsSeen(ids.split('\n')).catch((err: unknown) => console.warn('[zen-sync] news/seen:', err))
    }
  }, [ids])
  if (items.length === 0) return null
  return (
    <div className="mb-3 p-2 border border-[var(--color-border)]" data-zen-garden-news="">
      <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider mb-1">New for your Gardens</div>
      {items.map((i) => (
        <div key={i.id} className="text-[11px] text-[var(--color-text-secondary)]" data-zen-garden-news-item={i.id}>
          {newsLine(i)}
        </div>
      ))}
    </div>
  )
}
