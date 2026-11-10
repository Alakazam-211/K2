import { useCallback, useEffect, useLayoutEffect, useRef, useState, type JSX, type KeyboardEvent, type ReactNode } from 'react'
import { Button } from '@/components/ui'
import { useRoom, useRoomProjects } from '@/components/Room/RoomContext'
import { formatRelativeTime } from '@/lib/format-relative-time'
import { useSessionViewChrome } from './sessionViewChrome'
import { useOverlayChatter } from './useOverlayChatter'
import { useThreadInputs } from './useThreadInputs'
import { threadFloatsIn } from './threadFloats'
import type { OverlayThreadItem } from './overlayThread'
import {
  THREAD_INPUT_KIND_LABEL,
  THREAD_PANEL_HINT,
  THREAD_PANEL_LABEL,
  chatterInOrder,
  chatterLine,
  chatterRoute,
  fullLocalTime,
  type ThreadInputRow,
  type ThreadInputTone,
  type ThreadPanelKind,
} from './threadPanels'

/** Seconds now, ticking every 30 s while `active` (the rows' "5m ago"). */
function useNowSec(active: boolean): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))
  useEffect(() => {
    if (!active) return
    setNow(Math.floor(Date.now() / 1000))
    const id = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000)
    return () => clearInterval(id)
  }, [active])
  return now
}

/** Thread view's Chatter or Heartbeats panel, in the column the terminal
 *  reveal slides out (`SplitSessionColumns`). A plain list for now (Rosson,
 *  2026-11-09: "until we can think of a better way to display it").
 *  `active`: open (not sliding shut), so it reads and listens. */
export function ThreadDrawerPanel({ kind, active }: { kind: ThreadPanelKind; active: boolean }): JSX.Element {
  const chrome = useSessionViewChrome()
  const ref = useRef<HTMLElement>(null)
  const label = THREAD_PANEL_LABEL[kind]
  const close = chrome?.setThreadPanel

  const onKeyDown = (e: KeyboardEvent<HTMLElement>): void => {
    if (e.key !== 'Escape' || e.defaultPrevented || !close) return
    e.preventDefault()
    e.stopPropagation()
    // Back to the button that opened it.
    const el = ref.current
    const toggle = el ? threadFloatsIn(el)?.querySelector<HTMLElement>(`[data-thread-panel-toggle="${kind}"]`) : null
    close(null)
    toggle?.focus()
  }

  return (
    <section
      ref={ref}
      data-testid="thread-drawer-panel"
      data-thread-panel={kind}
      aria-label={label}
      onKeyDown={onKeyDown}
      className="flex-1 min-h-0 min-w-0 flex flex-col"
    >
      <header className="flex-shrink-0 flex items-center gap-2 min-h-12 px-3 py-1.5 border-b border-[var(--color-border)]">
        <div className="min-w-0 flex-1">
          <h2 className="type-name text-[var(--color-text-primary)] truncate">{label}</h2>
          <p className="type-hint text-[var(--color-text-muted)] truncate">{THREAD_PANEL_HINT[kind]}</p>
        </div>
        {close ? (
          <button
            type="button"
            data-testid="thread-drawer-close"
            aria-label={`Close ${label}`}
            title={`Close ${label} (Esc)`}
            onClick={() => close(null)}
            className="no-drag flex-shrink-0 inline-flex items-center justify-center h-6 w-6 rounded-[var(--radius-field)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
              <path d="M6 6l12 12M18 6L6 18" />
            </svg>
          </button>
        ) : null}
      </header>
      {kind === 'chatter' ? (
        <ChatterList addr={chrome?.overlayAddr ?? ''} conversationId={chrome?.conversationId ?? null} active={active} />
      ) : (
        <InputsList cwd={chrome?.projectPath ?? ''} active={active} />
      )}
    </section>
  )
}

// ── Shared bits ─────────────────────────────────────────────────────────

function PanelNote({ children, testId }: { children: ReactNode; testId: string }): JSX.Element {
  return (
    <div data-testid={testId} className="px-3 py-3 type-hint text-[var(--color-text-muted)]">
      {children}
    </div>
  )
}

function PanelError({ what, error, onRetry }: { what: string; error: string; onRetry: () => void }): JSX.Element {
  return (
    <div data-testid="thread-drawer-error" role="alert" className="px-3 py-3 flex flex-col items-start gap-2">
      <p className="type-hint text-[var(--color-text-secondary)]">
        Couldn’t load {what}. <span className="text-[var(--color-text-muted)] break-words">{error}</span>
      </p>
      <Button variant="quiet" onClick={onRetry}>
        Try again
      </Button>
    </div>
  )
}

// ── Chatter ─────────────────────────────────────────────────────────────

function ChatterList(props: { addr: string; conversationId: string | null; active: boolean }): JSX.Element {
  // A retry re-reads from scratch.
  const [attempt, setAttempt] = useState(0)
  return <ChatterListBody key={attempt} {...props} onRetry={() => setAttempt((n) => n + 1)} />
}

function ChatterListBody({
  addr,
  conversationId,
  active,
  onRetry,
}: {
  addr: string
  conversationId: string | null
  active: boolean
  onRetry: () => void
}): JSX.Element {
  const { items, error, hasMore, loadOlder, loadingOlder, loading } = useOverlayChatter({
    scope: useRoom().scope,
    addr,
    conversationId,
    enabled: active,
  })
  const nowSec = useNowSec(active)
  const rows = chatterInOrder(items)
  const listRef = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)
  const olderFrom = useRef<number | null>(null)
  const [open, setOpen] = useState<ReadonlySet<string>>(() => new Set())

  // Newest at the bottom: start there, and follow new messages while the
  // reader is there; loading older ones keeps the reader's place.
  useLayoutEffect(() => {
    const el = listRef.current
    if (!el) return
    if (olderFrom.current != null) {
      el.scrollTop += el.scrollHeight - olderFrom.current
      olderFrom.current = null
      return
    }
    if (pinned.current) el.scrollTop = el.scrollHeight
  }, [rows.length])

  const requestOlder = (): void => {
    if (!hasMore || loadingOlder) return
    if (listRef.current) olderFrom.current = listRef.current.scrollHeight
    void loadOlder()
  }

  const toggle = useCallback((id: string) => {
    setOpen((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }, [])

  let body: ReactNode
  if (!addr.trim()) {
    body = <PanelNote testId="thread-drawer-loading">Finding this agent’s address…</PanelNote>
  } else if (error && rows.length === 0) {
    body = <PanelError what="Chatter" error={error} onRetry={onRetry} />
  } else if (loading && rows.length === 0) {
    body = <PanelNote testId="thread-drawer-loading">Loading Chatter…</PanelNote>
  } else if (rows.length === 0) {
    body = (
      <PanelNote testId="thread-drawer-empty">
        No messages between agents yet. When this agent and another agent message each other, the messages show here,
        newest at the bottom.
      </PanelNote>
    )
  } else {
    body = (
      <>
        {hasMore ? (
          <div className="px-3 pt-2">
            <Button variant="quiet" data-testid="thread-drawer-older" disabled={loadingOlder} onClick={requestOlder} className="w-full">
              {loadingOlder ? 'Loading…' : 'Load older'}
            </Button>
          </div>
        ) : null}
        <ol className="flex flex-col py-1" data-testid="thread-drawer-list">
          {rows.map((it) => (
            <ChatterRow key={it.id} item={it} addr={addr} nowSec={nowSec} open={open.has(it.id)} onToggle={toggle} />
          ))}
        </ol>
      </>
    )
  }

  return (
    <div
      ref={listRef}
      className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden [scrollbar-gutter:stable]"
      onScroll={(e) => {
        const el = e.currentTarget
        pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
      }}
    >
      {body}
    </div>
  )
}

function ChatterRow({
  item,
  addr,
  nowSec,
  open,
  onToggle,
}: {
  item: OverlayThreadItem
  addr: string
  nowSec: number
  open: boolean
  onToggle: (id: string) => void
}): JSX.Element {
  const { from, to } = chatterRoute(item.doc)
  const at = item.doc.created_at
  const body = (item.doc.body ?? '').trim()
  const line = chatterLine(item.doc)
  // Nothing more to show when the line already is the whole body.
  const expandable = !!body && body !== line
  const who = (name: string): string =>
    name === addr ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'
  const head = (
    <>
      <span className="flex items-baseline gap-2 min-w-0">
        <span className="min-w-0 flex-1 truncate type-hint font-medium">
          <span className={who(from)}>{from}</span>
          {to ? (
            <>
              <span className="text-[var(--color-text-muted)]" aria-label="to">
                {' → '}
              </span>
              <span className={who(to)}>{to}</span>
            </>
          ) : null}
        </span>
        <time
          className="flex-shrink-0 type-hint text-[var(--color-text-muted)] tabular-nums"
          dateTime={at ? new Date(at * 1000).toISOString() : undefined}
          title={at ? fullLocalTime(at) : undefined}
        >
          {formatRelativeTime(at, nowSec)}
        </time>
      </span>
      <span
        className={`block type-hint text-[var(--color-text-secondary)] ${open ? 'whitespace-pre-wrap break-words' : 'truncate'}`}
        data-testid="thread-drawer-row-line"
      >
        {open ? body : line}
      </span>
    </>
  )
  return (
    <li data-testid="thread-drawer-row" data-kind="chatter" data-seq={item.seq} className="border-b border-[var(--color-border)] last:border-b-0">
      {expandable ? (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => onToggle(item.id)}
          className="w-full text-left flex flex-col gap-0.5 px-3 py-2 cursor-pointer hover:bg-[var(--color-bg-hover)] select-text"
        >
          {head}
        </button>
      ) : (
        <div className="flex flex-col gap-0.5 px-3 py-2">{head}</div>
      )}
    </li>
  )
}

// ── Heartbeats and outside input ────────────────────────────────────────

const TONE_CLASS: Record<ThreadInputTone, string> = {
  ok: 'text-[var(--color-status-ok-soft)]',
  warn: 'text-[var(--color-status-warn-amber)]',
  error: 'text-[var(--color-status-error-soft)]',
  muted: 'text-[var(--color-text-muted)]',
}

function InputsList({ cwd, active }: { cwd: string; active: boolean }): JSX.Element {
  // A tab's cwd can be one of the workspace's worktrees: read by the
  // workspace's own path.
  const projectPath =
    useRoomProjects(
      useCallback(
        (projects) =>
          cwd ? projects.find((p) => p.path === cwd || p.workspaces?.some((w) => w.worktreePath === cwd))?.path ?? null : null,
        [cwd],
      ),
    ) ?? cwd
  const { rows, loading, error, refresh } = useThreadInputs({ scope: useRoom().scope, projectPath, enabled: active })
  const nowSec = useNowSec(active)
  const listRef = useRef<HTMLDivElement>(null)
  const pinned = useRef(true)

  // Newest at the bottom, as in Chatter: start there, and follow new runs
  // while the reader is there.
  useLayoutEffect(() => {
    const el = listRef.current
    if (el && pinned.current) el.scrollTop = el.scrollHeight
  }, [rows.length])

  let body: ReactNode
  if (!projectPath.trim()) {
    body = <PanelNote testId="thread-drawer-empty">This chat isn’t in a workspace, so it has no heartbeats.</PanelNote>
  } else if (error && rows.length === 0) {
    body = <PanelError what="heartbeats" error={error} onRetry={refresh} />
  } else if (loading && rows.length === 0) {
    body = <PanelNote testId="thread-drawer-loading">Loading heartbeats…</PanelNote>
  } else if (rows.length === 0) {
    body = (
      <PanelNote testId="thread-drawer-empty">
        No heartbeats have run yet. Each time a heartbeat wakes an agent in this workspace, it shows here, newest at the bottom.
      </PanelNote>
    )
  } else {
    body = (
      <ol className="flex flex-col py-1" data-testid="thread-drawer-list">
        {rows.map((row) => (
          <InputRow key={row.id} row={row} nowSec={nowSec} />
        ))}
      </ol>
    )
  }

  return (
    <div className="flex-1 min-h-0 flex flex-col">
      <div
        ref={listRef}
        className="flex-1 min-h-0 overflow-y-auto overflow-x-hidden [scrollbar-gutter:stable]"
        onScroll={(e) => {
          const el = e.currentTarget
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24
        }}
      >
        {body}
      </div>
      <p
        data-testid="thread-drawer-coming"
        className="flex-shrink-0 px-3 py-2.5 type-hint text-[var(--color-text-muted)] border-t border-[var(--color-border)]"
      >
        Reminders and mail from your VIP senders will show here too, once K2 has them.
      </p>
    </div>
  )
}

function InputRow({ row, nowSec }: { row: ThreadInputRow; nowSec: number }): JSX.Element {
  return (
    <li
      data-testid="thread-drawer-row"
      data-kind={row.kind}
      className="flex flex-col gap-0.5 px-3 py-2 border-b border-[var(--color-border)] last:border-b-0"
      title={row.detail ?? undefined}
    >
      <span className="flex items-baseline gap-2 min-w-0">
        <span className="flex-shrink-0 type-badge uppercase tracking-wider text-[var(--color-text-muted)]">
          {THREAD_INPUT_KIND_LABEL[row.kind]}
        </span>
        <span className="min-w-0 flex-1 truncate type-hint font-medium text-[var(--color-text-primary)]">{row.source}</span>
        <time
          className="flex-shrink-0 type-hint text-[var(--color-text-muted)] tabular-nums"
          dateTime={new Date(row.at * 1000).toISOString()}
          title={fullLocalTime(row.at)}
        >
          {formatRelativeTime(row.at, nowSec)}
        </time>
      </span>
      <span className="block type-hint truncate" data-testid="thread-drawer-row-line">
        <span className={TONE_CLASS[row.tone]}>{row.status}</span>
        {row.detail ? <span className="text-[var(--color-text-muted)]"> · {row.detail}</span> : null}
      </span>
    </li>
  )
}
