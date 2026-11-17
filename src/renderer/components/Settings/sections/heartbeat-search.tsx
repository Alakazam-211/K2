// Search on Settings → Logs → Heartbeats: each word of the search, in any
// case, must appear somewhere in a heartbeat's name, its workspace, its
// schedule (with the days and months spelled out, so "monday" finds "Mon"),
// its state words, where it runs, or its instructions file (WAKEUP.md).
// The instructions are read only once someone searches, a few at a time,
// from the window's server, and kept for as long as the page is open, so
// name matches show at once and instruction matches join as they load.

import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { deriveDeliveryTarget, type HeartbeatDeliveryFields } from '@/lib/heartbeat-delivery'
import type { ServerScope } from '@/kessel/server-scope'
import { Button, SearchField, Segmented } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingDropdown } from '../controls/SettingControls'

/** What search needs of a heartbeat row. */
export interface SearchableHeartbeat extends HeartbeatDeliveryFields {
  id: string
  name: string
  projectId: string
  projectName: string
  projectPath: string
  wakeupPath: string
  frequency: string
  specJson: string
  enabled: boolean
  disabledReason: string | null
  consecutiveFailures: number
  scheduleError: string | null
}

export type HeartbeatFilter = 'all' | 'on' | 'paused' | 'attention'

/** The words of a search, lower case. */
export function queryWords(q: string): string[] {
  return q.toLowerCase().split(/\s+/).filter(Boolean)
}

/** Failing, or paused by K2 rather than by a person. */
export function needsAttention(row: SearchableHeartbeat): boolean {
  const byK2 = !row.enabled && !!row.disabledReason && row.disabledReason !== 'user'
  return byK2 || row.consecutiveFailures > 0 || !!row.scheduleError
}

export function inFilter(row: SearchableHeartbeat, f: HeartbeatFilter): boolean {
  if (f === 'on') return row.enabled
  if (f === 'paused') return !row.enabled
  if (f === 'attention') return needsAttention(row)
  return true
}

function stateWords(row: SearchableHeartbeat): string {
  const words = [row.enabled ? 'on enabled running' : 'paused off disabled']
  if (needsAttention(row)) words.push('needs attention failing failed problem')
  if (!row.enabled && row.disabledReason && row.disabledReason !== 'user') words.push('paused by k2')
  return words.join(' ')
}

const DAYS: Record<string, string> = { mon: 'monday', tue: 'tuesday', wed: 'wednesday', thu: 'thursday', fri: 'friday', sat: 'saturday', sun: 'sunday' }
const MONTHS: Record<string, string> = {
  jan: 'january', feb: 'february', mar: 'march', apr: 'april', may: 'may', jun: 'june',
  jul: 'july', aug: 'august', sep: 'september', oct: 'october', nov: 'november', dec: 'december',
}

function scheduleWords(row: SearchableHeartbeat, scheduleText: string): string {
  const words = [scheduleText, row.frequency]
  try {
    const spec = JSON.parse(row.specJson) as { days?: unknown; months?: unknown }
    for (const d of Array.isArray(spec.days) ? spec.days : []) if (typeof d === 'string' && DAYS[d]) words.push(DAYS[d]!)
    for (const m of Array.isArray(spec.months) ? spec.months : []) if (typeof m === 'string' && MONTHS[m]) words.push(MONTHS[m]!)
  } catch {
    /* the text alone */
  }
  return words.join(' ')
}

function runsInWords(row: SearchableHeartbeat): string {
  const mode = deriveDeliveryTarget(row).mode
  return mode === 'pinned' ? 'runs in pinned chat' : mode === 'session' ? 'runs in saved session' : 'runs in own session'
}

/** Where a heartbeat's instructions file is: stored relative to its
 *  workspace, or already absolute. */
export function instructionsPath(row: Pick<SearchableHeartbeat, 'projectPath' | 'wakeupPath'>): string {
  const p = row.wakeupPath
  return p.startsWith('/') || /^[A-Za-z]:[\\/]/.test(p) ? p : `${row.projectPath.replace(/\/+$/, '')}/${p.replace(/^\.?\//, '')}`
}

export interface RowMatch {
  /** Some word was found only in the instructions: show where. */
  snippet?: { text: string }
}

/** null = no match. Every word must appear in some field. */
export function matchHeartbeat(
  row: SearchableHeartbeat,
  scheduleText: string,
  instructions: string | undefined,
  words: string[],
): RowMatch | null {
  if (words.length === 0) return {}
  const meta = [row.name, row.projectName, scheduleWords(row, scheduleText), stateWords(row), runsInWords(row)].join(' \n ').toLowerCase()
  const missing = words.filter((w) => !meta.includes(w))
  if (missing.length === 0) return {}
  const raw = (instructions ?? '').toLowerCase()
  const plain = plainText(instructions ?? '')
  const lower = plain.toLowerCase()
  if (!missing.every((w) => lower.includes(w) || raw.includes(w))) return null
  // The line shows the first word that the plain text has (a word only in
  // a link's address shows the start).
  return { snippet: { text: snippetAround(plain, missing.find((w) => lower.includes(w)) ?? missing[0]!) } }
}

/** Markdown as one line of plain text: no heading marks (a heading ends
 *  with a full stop), bullets or numbers, quote marks, code fences,
 *  emphasis or code ticks, and links and images keep only their text. */
export function plainText(md: string): string {
  const lines: string[] = []
  for (const raw of md.split(/\r?\n/)) {
    let line = raw.trim()
    if (!line || /^(```|~~~)/.test(line)) continue
    const heading = /^#{1,6}\s+/.test(line)
    line = line
      .replace(/^#{1,6}\s+/, '')
      .replace(/^>\s?/, '')
      .replace(/^([-*+]|\d+[.)])\s+/, '')
      .replace(/!?\[([^\]]*)\]\([^)]*\)/g, '$1')
      .replace(/\[([^\]]*)\]\[[^\]]*\]/g, '$1')
      .replace(/`([^`]*)`/g, '$1')
      .replace(/(\*\*|__)(.+?)\1/g, '$2')
      .replace(/(^|[^\w*])\*([^*\s][^*]*?)\*(?=[^\w*]|$)/g, '$1$2')
      .replace(/(^|[^\w])_([^_\s][^_]*?)_(?=[^\w]|$)/g, '$1$2')
      .trim()
    if (heading && line && !/[.!?:]$/.test(line)) line += '.'
    if (line) lines.push(line)
  }
  return lines.join(' ').replace(/\s+/g, ' ').trim()
}

/** One line of `text` around the first `word`, with … where it was cut. */
export function snippetAround(text: string, word: string, radius = 48): string {
  const flat = text.replace(/\s+/g, ' ').trim()
  const at = flat.toLowerCase().indexOf(word.toLowerCase())
  if (at < 0) return flat.slice(0, radius * 2)
  // Cut between words, never inside one.
  let start = Math.max(0, at - radius)
  let end = Math.min(flat.length, at + word.length + radius)
  if (start > 0) {
    const sp = flat.indexOf(' ', start)
    start = sp >= 0 && sp < at ? sp + 1 : start
  }
  if (end < flat.length) {
    const sp = flat.lastIndexOf(' ', end)
    end = sp > at + word.length ? sp : end
  }
  return `${start > 0 ? '…' : ''}${flat.slice(start, end).trim()}${end < flat.length ? '…' : ''}`
}

/** `text` with each search word marked. */
export function Highlight({ text, words }: { text: string; words: string[] }): React.JSX.Element {
  if (words.length === 0) return <>{text}</>
  const re = new RegExp(`(${words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|')})`, 'gi')
  const parts = text.split(re)
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 ? (
          <mark key={i} data-search-hit="" className="bg-[color-mix(in_srgb,var(--color-accent)_28%,transparent)] text-[var(--color-text-primary)]">
            {p}
          </mark>
        ) : (
          <React.Fragment key={i}>{p}</React.Fragment>
        ),
      )}
    </>
  )
}

const BATCH = 4

/** Each heartbeat's instructions, read from `scope` the first time `active`
 *  is true (someone searched), a few at a time, and kept until the page
 *  closes. `forget(id)` drops one (its file was just edited) and reads it
 *  again while a search is on. */
export function useHeartbeatInstructions(
  rows: SearchableHeartbeat[] | null,
  active: boolean,
  scope: () => ServerScope,
): { texts: Record<string, string>; loading: boolean; forget: (id: string) => void } {
  const [texts, setTexts] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(false)
  const asked = useRef(new Set<string>())
  const [started, setStarted] = useState(false)
  const [nonce, setNonce] = useState(0)
  useEffect(() => {
    if (active) setStarted(true)
  }, [active])

  useEffect(() => {
    if (!started || !rows) return
    const todo = rows.filter((r) => !asked.current.has(r.id))
    if (todo.length === 0) return
    for (const r of todo) asked.current.add(r.id)
    const done = new Set<string>()
    let cancelled = false
    setLoading(true)
    void (async () => {
      for (let i = 0; i < todo.length && !cancelled; i += BATCH) {
        const batch = todo.slice(i, i + BATCH)
        const read = await Promise.all(
          batch.map(async (r) => {
            try {
              const res = await daemonCliGet<{ content?: unknown }>(scope(), 'fs/read-file', { path: instructionsPath(r) })
              return [r.id, typeof res?.content === 'string' ? res.content : ''] as const
            } catch {
              return [r.id, ''] as const
            }
          }),
        )
        if (cancelled) return
        for (const [id] of read) done.add(id)
        setTexts((t) => ({ ...t, ...Object.fromEntries(read) }))
      }
      if (!cancelled) setLoading(false)
    })()
    return () => {
      cancelled = true
      setLoading(false)
      // Unfinished reads are asked again on the next run.
      for (const r of todo) if (!done.has(r.id)) asked.current.delete(r.id)
    }
  }, [started, rows, scope, nonce])

  const forget = useCallback((id: string) => {
    asked.current.delete(id)
    setTexts((t) => {
      const next = { ...t }
      delete next[id]
      return next
    })
    setNonce((n) => n + 1)
  }, [])

  return { texts, loading, forget }
}

/** The page's search: the words, the chip and the workspace, kept while
 *  the page is open, and the rows they leave, each with its match. */
export function useHeartbeatSearch<R extends SearchableHeartbeat>(
  rows: R[] | null,
  scheduleText: (row: R) => string,
  scope: () => ServerScope,
) {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<HeartbeatFilter>('all')
  const [workspace, setWorkspace] = useState('all')
  const fieldRef = useRef<HTMLInputElement | null>(null)
  const words = useMemo(() => queryWords(query), [query])
  const { texts, loading, forget } = useHeartbeatInstructions(rows, words.length > 0, scope)

  // `/` focuses the search while the page is open, unless someone is
  // typing somewhere (⌘F stays the Settings search).
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey || !fieldRef.current) return
      const t = e.target as HTMLElement | null
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return
      e.preventDefault()
      fieldRef.current.focus()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  const workspaces = useMemo(() => {
    const seen = new Map<string, string>()
    for (const r of rows ?? []) if (!seen.has(r.projectId)) seen.set(r.projectId, r.projectName)
    return [...seen].map(([value, label]) => ({ value, label }))
  }, [rows])

  const matched = useMemo(
    () =>
      (rows ?? [])
        .filter((r) => workspace === 'all' || r.projectId === workspace)
        .map((row) => ({ row, match: matchHeartbeat(row, scheduleText(row), texts[row.id], words) }))
        .filter((x): x is { row: R; match: RowMatch } => x.match !== null),
    [rows, workspace, scheduleText, texts, words],
  )
  const counts: Record<HeartbeatFilter, number> = {
    all: matched.length,
    on: matched.filter((x) => inFilter(x.row, 'on')).length,
    paused: matched.filter((x) => inFilter(x.row, 'paused')).length,
    attention: matched.filter((x) => inFilter(x.row, 'attention')).length,
  }
  const visible = matched.filter((x) => inFilter(x.row, filter))
  const clear = (): void => {
    setQuery('')
    setFilter('all')
    setWorkspace('all')
  }
  return { query, setQuery, filter, setFilter, workspace, setWorkspace, words, fieldRef, workspaces, counts, visible, loading, forget, clear }
}

type Search = ReturnType<typeof useHeartbeatSearch>

/** The field, the chips and the workspace picker, over the list. */
export function HeartbeatSearchBar({ search }: { search: Search }): React.JSX.Element {
  const { counts } = search
  return (
    <div className="px-4 py-3 space-y-3" data-testid="heartbeats-search">
      <SearchField
        ref={search.fieldRef}
        value={search.query}
        onChange={(e) => search.setQuery(e.target.value)}
        placeholder="Search heartbeats"
        aria-label="Search heartbeats"
        className="w-full"
        data-settings-id="wake-scheduler.search"
      />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Segmented<HeartbeatFilter>
          aria-label="Show heartbeats"
          value={search.filter}
          onChange={search.setFilter}
          options={[
            { value: 'all', label: `All (${counts.all})` },
            { value: 'on', label: `On (${counts.on})` },
            { value: 'paused', label: `Paused (${counts.paused})` },
            { value: 'attention', label: `Needs attention (${counts.attention})` },
          ]}
        />
        {search.workspaces.length > 1 && (
          <SettingDropdown
            ariaLabel="Workspace"
            value={search.workspace}
            onChange={search.setWorkspace}
            menuAlign="left"
            options={[{ value: 'all', label: 'All workspaces' }, ...search.workspaces]}
          />
        )}
        {search.loading && (
          <span role="status" className="type-hint text-[var(--color-text-secondary)]">
            Searching instructions…
          </span>
        )}
      </div>
    </div>
  )
}

/** Nothing left after the search or the filters. */
export function HeartbeatNoMatch({ search }: { search: Search }): React.JSX.Element {
  const q = search.query.trim()
  return (
    <div className="px-4 py-6 flex items-start justify-between gap-6" data-testid="heartbeats-no-match">
      <div className="min-w-0">
        <p className={SETTING_ROW_LABEL}>{q ? `No heartbeats match “${q}”.` : 'No heartbeats match these filters.'}</p>
        {search.loading && <p className={SETTING_ROW_HINT}>Still reading instructions; more may show.</p>}
      </div>
      <Button onClick={search.clear}>Clear search</Button>
    </div>
  )
}

/** Under a row found only by its instructions: the line it was found in. */
export function InstructionsSnippet({ id, text, words }: { id: string; text: string; words: string[] }): React.JSX.Element {
  return (
    <p className={`${SETTING_ROW_HINT} truncate`} data-testid={`heartbeat-snippet-${id}`}>
      Instructions: <Highlight text={text} words={words} />
    </p>
  )
}
