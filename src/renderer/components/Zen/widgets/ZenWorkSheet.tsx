// 0.46.7 Workbench: K2's own sheet for a widget's Start, Land, Retire or
// Discard (GTC-5). K2's DOM laid over the widget's card, outside the sealed
// frame (the G8 pattern, like ZenLinkSheet): the widget can't draw it, click
// it or read it, and every word is K2's; the facts (commits, files, branch)
// are the daemon's, read when the person pressed the widget's button.
//
// Cancel (also Esc) answers the widget `{cancelled: true}` (Start:
// `declined`); the main button runs the change through the daemon.

import { useEffect, useRef } from 'react'
import type { ZenWorkChange } from '@/lib/zen/zen-custom-work'
import { zenButtonStyle } from './ZenOverlay'

const muted: React.CSSProperties = { color: 'var(--zen-text-muted)' }
const mono: React.CSSProperties = { fontFamily: 'var(--zen-font-mono, ui-monospace, monospace)', fontSize: 11 }

export interface ZenWorkSheetProps {
  change: ZenWorkChange
  widget: string
  onDone: (yes: boolean) => void
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : typeof v === 'number' ? String(v) : ''
}

function list(v: unknown): Array<Record<string, unknown>> {
  return Array.isArray(v) ? v.filter((x): x is Record<string, unknown> => x !== null && typeof x === 'object') : []
}

/** The sheet's title and main button for a change (exported for tests). */
export function zenWorkSheetWords(c: ZenWorkChange): { title: string; go: string; danger: boolean } {
  const who = str(c.name) || str(c.thread)
  if (c.kind === 'land') return { title: `Land ${who}’s ${str(c.ahead)} commit(s) on ${str(c.base)}?`, go: `Land on ${str(c.base)}`, danger: false }
  if (c.kind === 'settle' && c.discard === true) return { title: `Discard ${who}?`, go: 'Discard', danger: true }
  if (c.kind === 'settle') return { title: `Retire ${who}?`, go: 'Retire', danger: false }
  return { title: `Start ${who} in ${str(c.workspace)}?`, go: 'Start', danger: false }
}

export function ZenWorkSheet({ change, widget, onDone }: ZenWorkSheetProps): React.JSX.Element {
  const cancel = useRef<HTMLButtonElement | null>(null)
  useEffect(() => {
    cancel.current?.focus()
  }, [])
  const words = zenWorkSheetWords(change)
  const commits = list(change.commits)
  const files = list(change.files)
  const uncommitted = typeof change.uncommitted === 'number' ? change.uncommitted : 0
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label={words.title}
      data-testid="zen-work-sheet"
      data-kind={change.kind}
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onDone(false)
        }
      }}
    >
      <div
        className="w-full"
        style={{
          maxWidth: 420,
          maxHeight: '100%',
          overflow: 'auto',
          padding: 12,
          fontSize: 12,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
        }}
      >
        <p style={{ fontWeight: 500 }} data-testid="zen-work-title">
          {words.title}
        </p>
        <p style={{ ...muted, fontSize: 11 }}>Asked by the widget {widget}. Only your press here does it.</p>
        {change.kind === 'land' && (
          <>
            <p style={muted}>
              Branch <span style={mono}>{str(change.branch)}</span> → <span style={mono}>{str(change.base)}</span> (at{' '}
              <span style={mono}>{str(change.onto)}</span>), cherry-picked; a conflict lands nothing and tells the agent.
            </p>
            <ul data-testid="zen-work-commits" style={{ margin: 0, paddingLeft: 16 }}>
              {commits.slice(0, 12).map((c) => (
                <li key={str(c.sha)}>
                  <span style={mono}>{str(c.short)}</span> {str(c.subject)}
                </li>
              ))}
              {commits.length > 12 && <li style={muted}>and {commits.length - 12} more</li>}
            </ul>
            <p style={muted} data-testid="zen-work-files">
              {files.length} file(s): {files.slice(0, 8).map((f) => str(f.file)).join(', ')}
              {files.length > 8 ? ', …' : ''}
            </p>
            {uncommitted > 0 && <p style={muted}>{uncommitted} uncommitted file(s) stay in the agent’s worktree.</p>}
          </>
        )}
        {change.kind === 'settle' && (
          <p style={muted} data-testid="zen-work-settle">
            K2 stops the agent and removes its worktree {str(change.worktree) && <span style={mono}>{str(change.worktree)}</span>}
            {change.discard === true
              ? `: its ${str(change.unlanded) || '0'} commit(s) that aren’t landed are thrown away, and uncommitted files go to the Trash.`
              : ' and deletes its branch (every commit is landed).'}{' '}
            Its Thread stays.
          </p>
        )}
        {change.kind === 'start' && (
          <>
            <p style={muted}>
              A new worktree <span style={mono}>{str(change.worktree)}</span> on branch <span style={mono}>{str(change.branch)}</span> from{' '}
              <span style={mono}>{str(change.base)}</span>
              {str(change.harness) ? `, with ${str(change.harness)}` : ''}.
            </p>
            <p data-testid="zen-work-brief" style={{ ...muted, whiteSpace: 'pre-wrap', maxHeight: 120, overflow: 'auto' }}>
              {str(change.brief)}
            </p>
          </>
        )}
        <div className="flex flex-wrap" style={{ gap: 6 }}>
          <button ref={cancel} type="button" className="cursor-pointer" style={zenButtonStyle(false)} data-testid="zen-work-cancel" onClick={() => onDone(false)}>
            Cancel
          </button>
          <button type="button" className="cursor-pointer" style={zenButtonStyle(true)} data-testid="zen-work-go" onClick={() => onDone(true)}>
            {words.go}
          </button>
        </div>
      </div>
    </div>
  )
}
