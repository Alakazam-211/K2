// prd-zen-mode-v1 Z38, Z39, Z32 (answers 4, 5, 9) — the Agents widget: the
// Home's agents like a list of chats.
//
// One row per Home row, in Home order (⌘1–9 selects row N through
// `registerZenRowSelect`, installed with the data verbs). Each row: avatar,
// name, the server when it isn't this computer, the agent's live status
// (working, needs you, idle — no status when its server can't say), and the
// start of its last message. No unread dots (answer 9). A row that can't be
// messaged says why ("Offline", "Sign in", "No access", "Update <server>").
// Selecting a row opens its conversation in place (`conversation.open`):
// never a window server switch. ⌘↑ / ⌘↓ move between conversations.
//
// Everything comes through the bridge (`agents:read`).

import { useEffect, useRef } from 'react'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenWidgetStyles, initials, shortAge, useNowSec, useZenRows } from './zen-widget-kit'

export const ZEN_EMPTY_HOME = 'This Home has no agents yet. Exit Zen to add some.'

const ACTIVITY_TEXT = { working: 'working', 'needs-you': 'needs you', idle: 'idle' } as const

/** The status mark on the avatar, and its word. */
export function ZenStatusDot({ row, size = 10 }: { row: ZenAgentRow; size?: number }): React.JSX.Element | null {
  if (row.activity === null) return null
  const color =
    row.activity === 'working' ? 'var(--zen-working)' : row.activity === 'needs-you' ? 'var(--zen-needs-you)' : 'transparent'
  return (
    <span
      aria-hidden
      data-zen-status-dot={row.activity}
      data-zen-pulse={row.activity === 'working' ? '' : undefined}
      style={{
        display: 'inline-block',
        width: size,
        height: size,
        borderRadius: 999,
        background: color,
        border: row.activity === 'idle' ? '1.5px solid var(--zen-text-muted)' : '2px solid var(--zen-surface)',
        boxSizing: 'border-box',
      }}
    />
  )
}

function AgentRow({
  row,
  nowSec,
  onOpen,
}: {
  row: ZenAgentRow
  nowSec: number
  onOpen(address: string): void
}): React.JSX.Element {
  const second = row.state !== 'ok'
    ? row.stateLabel
    : row.preview
      ? `${row.preview.mine ? 'You: ' : ''}${row.preview.text}`
      : null
  const muted = row.state !== 'ok'
  return (
    <li role="presentation">
      <button
        type="button"
        role="option"
        aria-selected={row.selected}
        title={row.detail ?? (row.server ? `${row.label} on ${row.server}` : row.label)}
        data-zen-agent-row={row.address}
        data-selected={row.selected ? '' : undefined}
        data-activity={row.activity ?? 'unknown'}
        data-state={row.state}
        onClick={() => onOpen(row.address)}
        className="flex w-full items-center gap-3 text-left"
        style={{
          padding: '10px 12px',
          borderRadius: 'var(--zen-radius)',
          color: 'var(--zen-text)',
          background: 'transparent',
          opacity: muted ? 0.72 : 1,
          cursor: 'default',
        }}
      >
        <span className="relative flex-shrink-0" style={{ width: 40, height: 40 }}>
          <span
            className="flex h-full w-full items-center justify-center"
            style={{
              borderRadius: 999,
              background: row.selected ? 'var(--zen-accent)' : 'var(--zen-bubble-agent)',
              color: row.selected ? 'var(--zen-accent-text)' : 'var(--zen-bubble-agent-text)',
              fontWeight: 600,
              fontSize: '0.95em',
            }}
          >
            {initials(row.label)}
          </span>
          <span className="absolute" style={{ right: -1, bottom: -1, lineHeight: 0 }}>
            <ZenStatusDot row={row} size={12} />
          </span>
        </span>
        <span className="flex min-w-0 flex-1 flex-col" style={{ gap: 2 }}>
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate" style={{ fontWeight: 600 }} data-zen-agent-name="">
              {row.label}
            </span>
            {row.server && (
              <span
                className="flex-shrink-0 truncate"
                data-zen-server-tag=""
                style={{
                  maxWidth: '9rem',
                  fontSize: '0.72em',
                  padding: '1px 7px',
                  borderRadius: 999,
                  border: '1px solid var(--zen-border)',
                  color: 'var(--zen-text-muted)',
                }}
              >
                {row.server}
              </span>
            )}
            <span className="ml-auto flex-shrink-0" style={{ fontSize: '0.75em', color: 'var(--zen-text-muted)' }}>
              {row.state === 'ok' && row.preview ? shortAge(row.preview.at, nowSec) : ''}
            </span>
          </span>
          <span className="flex min-w-0 items-center gap-2" style={{ fontSize: '0.86em' }}>
            <span
              className="min-w-0 flex-1 truncate"
              data-zen-preview={row.state === 'ok' && row.preview ? '' : undefined}
              style={{ color: 'var(--zen-text-muted)' }}
            >
              {second ?? ' '}
            </span>
            {row.activity !== null && row.state === 'ok' && (
              <span
                className="flex-shrink-0"
                data-zen-activity={row.activity}
                style={{
                  fontSize: '0.85em',
                  color:
                    row.activity === 'working'
                      ? 'var(--zen-working)'
                      : row.activity === 'needs-you'
                        ? 'var(--zen-needs-you)'
                        : 'var(--zen-text-muted)',
                  fontWeight: row.activity === 'idle' ? 400 : 600,
                }}
              >
                {ACTIVITY_TEXT[row.activity]}
              </span>
            )}
          </span>
        </span>
      </button>
    </li>
  )
}

export function ZenAgentsWidget({ bridge }: ZenWidgetProps): React.JSX.Element {
  const rows = useZenRows(bridge)
  const nowSec = useNowSec()
  const rowsRef = useRef(rows)
  rowsRef.current = rows
  const open = (address: string): void => {
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', address))
      .catch((err: unknown) => console.warn('[zen] open conversation failed:', err))
  }
  const openRef = useRef(open)
  openRef.current = open

  // Z32: ⌘↑ / ⌘↓ move between conversations.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (!e.metaKey || e.shiftKey || e.altKey || e.ctrlKey) return
      if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
      const list = rowsRef.current
      if (list.length === 0) return
      e.preventDefault()
      const at = list.findIndex((r) => r.selected)
      const next =
        at < 0 ? (e.key === 'ArrowDown' ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, at + (e.key === 'ArrowDown' ? 1 : -1)))
      if (next !== at) openRef.current(list[next].address)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div className="flex h-full min-h-0 w-full flex-col" data-zen-widget="agents">
      <ZenWidgetStyles />
      <div
        className="flex-shrink-0"
        style={{
          padding: '14px 18px 6px',
          fontSize: '0.78em',
          fontWeight: 600,
          letterSpacing: '0.04em',
          textTransform: 'uppercase',
          color: 'var(--zen-text-muted)',
        }}
      >
        Agents
      </div>
      {rows.length === 0 ? (
        <div
          className="flex flex-1 items-center justify-center text-center"
          data-zen-agents-empty=""
          style={{ padding: 24, color: 'var(--zen-text-muted)' }}
        >
          {ZEN_EMPTY_HOME}
        </div>
      ) : (
        <ul
          role="listbox"
          aria-label="Agents"
          className="flex min-h-0 flex-1 flex-col overflow-y-auto"
          style={{ padding: '2px 8px 10px', gap: 2 }}
        >
          {rows.map((row) => (
            <AgentRow key={row.address} row={row} nowSec={nowSec} onOpen={open} />
          ))}
        </ul>
      )}
    </div>
  )
}
