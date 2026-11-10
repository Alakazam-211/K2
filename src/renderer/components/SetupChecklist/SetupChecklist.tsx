// Setup checklist (prd-setup-checklist-v1): the setup a person still has
// to do on the window's server, kept with the main navigation until it's
// done or hidden. The daemon decides every row and who sees it
// (`GET /cli/first-run/status`); this draws that answer. Each row opens the
// step itself, each row can be skipped, and the whole list can be hidden.
//
// Shown in the Agents sidebar and the Home sidebar; the collapsed rails get
// `SetupChecklistRailButton`, a count of the rows left that opens the nav.
// Nothing renders while the server's switch is off, or once the person hid
// or finished it.
//
// Look: ui/ primitives and theme tokens only, so Terminal and Glass style
// it through CSS. K2 Design polishes it before the switch goes on.

import { useEffect } from 'react'
import { Button, Tag, cx } from '@/components/ui'
import type { TagTone } from '@/components/ui'
import { primaryScope } from '@/kessel/server-scope'
import { openFirstRunTarget } from '@/lib/first-run-openers'
import { useSidebarStore } from '@/stores/sidebar'
import {
  rowsLeft,
  useFirstRunStore,
  visibleRows,
  watchFirstRun,
  type FirstRunRow,
  type FirstRunRowId,
  type FirstRunRowState,
} from '@/stores/first-run'

/** The contract's copy: what each step is for, never where or how it looks. */
export const SETUP_CHECKLIST_COPY: Record<FirstRunRowId, { label: string; line: string }> = {
  llm_sign_in: { label: 'Sign in to Claude, Codex or Grok', line: 'Your agents think with this account.' },
  first_agent: { label: 'Make your first agent', line: 'Each workspace is an agent with its own folder.' },
  tunnel: { label: 'Turn on the tunnel', line: 'Reach this server from anywhere.' },
  phone: { label: 'Connect your phone', line: 'Talk to your agents from your phone.' },
  own_sign_in: { label: 'Make your own sign-in', line: 'Sign in from your phone and your other devices.' },
}

export const NEEDS_TUNNEL_NOTE = 'Away from home, your phone reaches this server through the tunnel, so that comes first.'

export const CHECKLIST_TITLE = 'Setup checklist'

const STATE_TEXT: Record<FirstRunRowState, string> = { done: 'Done', todo: 'To do', skipped: 'Skipped' }
const STATE_TONE: Record<FirstRunRowState, TagTone> = { done: 'ok', todo: 'neutral', skipped: 'muted' }

/** The window's server's checklist rows, watched while mounted (null when
 *  nothing should show). */
export function useSetupChecklistRows(): { rows: FirstRunRow[] | null; error: string | null } {
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  const entry = useFirstRunStore((s) => s.entries[scope.id])
  return { rows: visibleRows(entry?.doc), error: entry?.error ?? null }
}

function leftText(n: number): string {
  return n === 1 ? '1 left' : `${n} left`
}

function ChecklistRow({ row }: { row: FirstRunRow }): React.JSX.Element {
  const copy = SETUP_CHECKLIST_COPY[row.id]
  return (
    <li className="flex items-start gap-1" data-setup-row={row.id} data-state={row.state}>
      <button
        type="button"
        data-testid={`setup-row-${row.id}`}
        className="k2-focus no-drag flex-1 min-w-0 text-left px-1.5 py-1 cursor-pointer hover:bg-[var(--color-bg-hover)] disabled:cursor-default disabled:hover:bg-transparent"
        disabled={row.opens === null}
        onClick={() => void openFirstRunTarget(row.opens)}
      >
        <span className="flex items-center gap-1.5">
          <span
            className={cx(
              'type-row min-w-0 truncate',
              row.state === 'todo' ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)]',
            )}
          >
            {copy.label}
          </span>
          <Tag tone={STATE_TONE[row.state]} className="flex-shrink-0">
            {STATE_TEXT[row.state]}
          </Tag>
        </span>
        <span className="block type-hint text-[var(--color-text-muted)] leading-snug">{copy.line}</span>
        {row.note === 'needs_tunnel' && row.state === 'todo' && (
          <span className="block type-hint text-[var(--color-text-secondary)] leading-snug mt-0.5" data-setup-note={row.note}>
            {NEEDS_TUNNEL_NOTE}
          </span>
        )}
      </button>
      {row.state === 'todo' && (
        <Button
          variant="quiet"
          className="flex-shrink-0 mt-1 px-1.5"
          aria-label={`Skip: ${copy.label}`}
          data-testid={`setup-skip-${row.id}`}
          onClick={() => void useFirstRunStore.getState().skip(row.id)}
        >
          Skip
        </Button>
      )}
    </li>
  )
}

/** The checklist, for a sidebar. Renders nothing when there's nothing to do. */
export function SetupChecklist({ className }: { className?: string }): React.JSX.Element | null {
  const { rows, error } = useSetupChecklistRows()
  if (!rows) return null
  const left = rowsLeft(rows)
  return (
    <section
      data-setup-checklist=""
      aria-labelledby="setup-checklist-title"
      className={cx('px-3 py-2 border-t border-[var(--color-border)] min-h-0 flex-shrink overflow-y-auto max-h-[45%]', className)}
    >
      <div className="flex items-center justify-between gap-2 mb-1">
        <h2 id="setup-checklist-title" className="m-0 type-row font-medium text-[var(--color-text-primary)]">
          {CHECKLIST_TITLE}
          <span className="ml-2 type-hint font-normal text-[var(--color-text-muted)]" data-testid="setup-left">
            {leftText(left)}
          </span>
        </h2>
        <Button
          variant="quiet"
          className="flex-shrink-0 px-1.5"
          aria-label="Hide the setup checklist"
          data-testid="setup-hide"
          onClick={() => void useFirstRunStore.getState().hide()}
        >
          Hide
        </Button>
      </div>
      <ul className="m-0 p-0 list-none flex flex-col gap-0.5">
        {rows.map((row) => (
          <ChecklistRow key={row.id} row={row} />
        ))}
      </ul>
      {error && (
        <p role="alert" className="m-0 mt-1 type-hint text-[var(--color-status-error-soft)]">
          {error}
        </p>
      )}
    </section>
  )
}

/** The collapsed rail's counterpart: how many rows are left; opens the nav,
 *  where the checklist is. Nothing when none are left. */
export function SetupChecklistRailButton(): React.JSX.Element | null {
  const { rows } = useSetupChecklistRows()
  if (!rows) return null
  const left = rowsLeft(rows)
  if (left === 0) return null
  const label = `${CHECKLIST_TITLE}: ${leftText(left)}`
  return (
    <button
      type="button"
      data-material="frost"
      data-frost-shape="round"
      data-nav-control=""
      data-testid="setup-rail"
      className="k2-focus no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 type-hint font-medium text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors"
      title={label}
      aria-label={label}
      onClick={() => useSidebarStore.getState().expand()}
    >
      {left}
    </button>
  )
}
