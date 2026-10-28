// Settings → Gardens → Local Data, the 0.46.6 parts (prd-garden-board-needs-v1
// GB17, GH6, R2b): a widget's files line (bytes and quota), "Export with
// images" (a zip of data.jsonl plus blobs/), and Restore… (the history list,
// a point to go back to, a key family or Everything, a dry run first, and
// the warning when a whole-widget restore rewrites more than one family).
// Every call is the person's, on THIS computer's daemon.

import React, { useEffect, useState } from 'react'
import {
  gardenHistory,
  listGardenBlobs,
  readGardenBlob,
  restoreGardenData,
  type GardenHistoryItem,
  type GardenRestored,
} from '@/lib/garden-data/garden-data-api'
import { zipStore, type ZipEntry } from '@/lib/zip-store'

const BTN =
  'px-2 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] transition-colors no-drag cursor-pointer disabled:opacity-40 disabled:cursor-default'
const MUTED = 'text-[11px] text-[var(--color-text-muted)]'

/** R2b: the warning before a whole-widget restore that rewrites more than
 *  one top-level key family. */
export const RESTORE_WIDE_WARNING =
  'This also changes settings and projects. If this widget keeps who-can-see-what in its data, access you removed may come back.'

function mib(n: number): string {
  return n >= 1024 * 1024 ? `${(n / (1024 * 1024)).toFixed(1)} MiB` : `${Math.max(1, Math.round(n / 1024))} KiB`
}

/** "Files: 3.2 MiB of 256 MiB" for a widget, or nothing while it has none. */
export function ZenWidgetFilesLine({ widget, rev }: { widget: string; rev: number }): React.JSX.Element | null {
  const [usage, setUsage] = useState<{ bytes: number; quota: number; count: number } | null>(null)
  useEffect(() => {
    let live = true
    listGardenBlobs(widget, { limit: 500 })
      .then((p) => live && setUsage({ bytes: p.bytes, quota: p.quota, count: p.items.length }))
      .catch((err: unknown) => console.warn(`[settings] ${widget}'s files:`, err))
    return () => {
      live = false
    }
  }, [widget, rev])
  if (!usage || usage.count === 0) return null
  return (
    <div className={MUTED} data-garden-data-files={widget}>
      Files: {mib(usage.bytes)} of {mib(usage.quota)}
    </div>
  )
}

function base64Bytes(b64: string): Uint8Array {
  const bin = atob(b64)
  const out = new Uint8Array(bin.length)
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i)
  return out
}

const CHUNK = 8 * 1024 * 1024

/** GB17 / Q12: `data.jsonl` plus every file a key names, as `blobs/<hash>`
 *  (the same layout `k2 zen data export --blobs <dir>` writes). */
export async function zenDataZip(widget: string, jsonl: () => Promise<string>): Promise<Uint8Array> {
  const entries: ZipEntry[] = [{ name: 'data.jsonl', bytes: new TextEncoder().encode(await jsonl()) }]
  let after: string | undefined
  for (;;) {
    const page = await listGardenBlobs(widget, after ? { after, limit: 500 } : { limit: 500 })
    for (const b of page.items.filter((i) => i.refs > 0)) {
      const hash = b.ref.replace(/^k2blob:sha256-/, '')
      if (!/^[0-9a-f]{64}$/.test(hash)) throw new Error(`a file ref K2 doesn't know: ${b.ref}`)
      const bytes = new Uint8Array(b.size)
      let at = 0
      for (;;) {
        const s = await readGardenBlob(widget, b.ref, at, CHUNK)
        const part = base64Bytes(s.base64)
        bytes.set(part, at)
        at += part.length
        if (s.eof) break
        if (part.length === 0) throw new Error(`${b.ref} ended early`)
      }
      if (at !== b.size) throw new Error(`${b.ref} is ${at} bytes, not ${b.size}`)
      entries.push({ name: `blobs/${hash}`, bytes })
    }
    if (!page.next) break
    after = page.next
  }
  return zipStore(entries)
}

/** Save bytes with the renderer's anchor download (as the JSONL export). */
export function saveBytes(name: string, bytes: Uint8Array, type: string): void {
  const url = URL.createObjectURL(new Blob([bytes], { type }))
  const a = document.createElement('a')
  a.href = url
  a.download = name
  a.click()
  URL.revokeObjectURL(url)
}

function when(at: number): string {
  return new Date(at * 1000).toLocaleString()
}

/** Restore…: pick a change to go back before, a family (or Everything),
 *  see the dry run, then confirm. */
export function ZenWidgetRestorePanel({ widget, onDone }: { widget: string; onDone: () => void }): React.JSX.Element {
  const [items, setItems] = useState<GardenHistoryItem[] | null>(null)
  const [oldest, setOldest] = useState(0)
  const [toRev, setToRev] = useState<number | null>(null)
  const [family, setFamily] = useState<string>('') // '' = Everything
  const [dry, setDry] = useState<GardenRestored | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<string | null>(null)

  useEffect(() => {
    let live = true
    gardenHistory(widget, { limit: 100 })
      .then((h) => {
        if (!live) return
        setItems(h.items)
        setOldest(h.oldest)
      })
      .catch((err: unknown) => live && setError(err instanceof Error ? err.message : String(err)))
    return () => {
      live = false
    }
  }, [widget])

  // The dry run for the chosen point and family.
  useEffect(() => {
    if (toRev === null) return
    let live = true
    setDry(null)
    setError(null)
    restoreGardenData({ widget, toRev, dryRun: true, ...(family ? { prefixes: [family] } : {}) })
      .then((d) => live && setDry(d))
      .catch((err: unknown) => live && setError(err instanceof Error ? err.message : String(err)))
    return () => {
      live = false
    }
  }, [widget, toRev, family])

  // The families a whole-widget restore to this point rewrites.
  const [allFamilies, setAllFamilies] = useState<string[]>([])
  useEffect(() => {
    if (toRev === null) return
    let live = true
    restoreGardenData({ widget, toRev, dryRun: true })
      .then((d) => live && setAllFamilies(d.families))
      .catch(() => live && setAllFamilies([]))
    return () => {
      live = false
    }
  }, [widget, toRev])

  const wide = family === '' && dry !== null && dry.families.length > 1
  const changes = dry ? dry.written + dry.deleted : 0

  return (
    <div className="mt-2 p-2 border border-[var(--color-border)]" data-garden-data-restore={widget}>
      <div className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">Restore</div>
      <p className={MUTED}>
        Go back to how {widget}&apos;s data was before a change. K2 keeps 7 days of changes, takes a snapshot first, and the restore is
        itself a change you can undo.
      </p>
      {items === null && !error ? <p className={MUTED}>Reading the history…</p> : null}
      {items && items.length === 0 ? <p className={MUTED}>No changes kept.</p> : null}
      {items && items.length > 0 ? (
        <div className="max-h-48 overflow-auto mt-1" data-garden-data-history="">
          {items.map((i) => {
            const can = i.rev - 1 >= oldest
            return (
              <label key={`${i.rev}/${i.key}`} className={`flex items-center gap-2 text-[11px] ${can ? '' : 'opacity-40'}`}>
                <input
                  type="radio"
                  name={`restore-${widget}`}
                  disabled={!can || busy}
                  checked={toRev === i.rev - 1}
                  onChange={() => {
                    setDone(null)
                    setToRev(i.rev - 1)
                  }}
                />
                <span className="truncate">
                  {when(i.at)} · {i.deleted ? 'deleted' : i.prevKind === 'none' ? 'made' : 'changed'} {i.key} · {i.by}
                  {i.restore ? ' · a restore' : ''}
                </span>
              </label>
            )
          })}
        </div>
      ) : null}
      {toRev !== null ? (
        <div className="mt-2 flex items-center gap-2 text-[11px]">
          <span>Restore</span>
          <select
            className="text-[11px] bg-transparent border border-[var(--color-border)]"
            value={family}
            onChange={(e) => setFamily(e.target.value)}
            data-garden-data-family=""
          >
            <option value="">Everything</option>
            {allFamilies.map((f) => (
              <option key={f} value={f}>
                {f}
              </option>
            ))}
          </select>
          <span className={MUTED}>
            {dry ? (changes === 0 ? 'Nothing to change.' : `${changes} key${changes === 1 ? '' : 's'} change: ${dry.families.join(', ')}`) : '…'}
          </span>
        </div>
      ) : null}
      {wide ? (
        <p className="text-[11px] text-[var(--color-status-warn)] mt-1" data-garden-data-restore-warning="">
          {RESTORE_WIDE_WARNING}
        </p>
      ) : null}
      <div className="mt-2 flex gap-1">
        <button
          type="button"
          className={BTN}
          disabled={busy || toRev === null || !dry || changes === 0}
          onClick={async () => {
            if (toRev === null) return
            setBusy(true)
            setError(null)
            try {
              const r = await restoreGardenData({ widget, toRev, ...(family ? { prefixes: [family] } : {}) })
              setDone(r.rev === null ? 'Nothing to change.' : `Restored: ${r.written + r.deleted} keys, as one change you can undo.`)
              onDone()
            } catch (err) {
              setError(err instanceof Error ? err.message : String(err))
            } finally {
              setBusy(false)
            }
          }}
        >
          Restore
        </button>
      </div>
      {done ? <p className={MUTED}>{done}</p> : null}
      {error ? (
        <p className="text-[11px] text-[var(--color-status-error)] mt-1" data-testid="garden-data-restore-error">
          {error}
        </p>
      ) : null}
    </div>
  )
}
