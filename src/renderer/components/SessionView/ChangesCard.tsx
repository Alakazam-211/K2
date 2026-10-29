// K2's changed-files card for one agent turn (workbench-v1 B7, vs-live E8).
//
// The daemon posts it from `k2` on a worktree agent's Thread at the end of
// a turn that changed files or made commits, read from git at the turn's
// start and end (never the agent's transcript). Read-only: a file list
// with a status letter and +/−, the commit count and the branch. The same
// card draws in K2's Thread and in a Garden's Conversation widget (both
// use ThreadBubble).

import type { JSX } from 'react'
import type { ThreadChangesDoc } from './overlayThread'

/** A file's status as one letter (git's own letters). */
export function changeLetter(status: string): string {
  switch (status) {
    case 'added':
    case 'untracked':
      return 'A'
    case 'deleted':
      return 'D'
    case 'renamed':
      return 'R'
    default:
      return 'M'
  }
}

/** The card's headline: "3 files changed · +41 −12 · 2 commits". */
export function changesHeadline(c: ThreadChangesDoc): string {
  const parts: string[] = []
  if (c.total > 0) parts.push(`${c.total} file${c.total === 1 ? '' : 's'} changed`)
  if (c.total > 0 && (c.additions > 0 || c.deletions > 0)) parts.push(`+${c.additions} −${c.deletions}`)
  if (c.commits > 0) parts.push(`${c.commits} commit${c.commits === 1 ? '' : 's'}`)
  if (parts.length === 0) parts.push('No file changes')
  return parts.join(' · ')
}

const STATUS_COLOR: Record<string, string> = {
  A: 'var(--color-success, #30a46c)',
  D: 'var(--color-danger, #e5484d)',
  R: 'var(--thread-card-muted, var(--color-text-muted))',
  M: 'var(--color-warning, #f5a524)',
}

export function ChangesCard({ changes }: { changes: ThreadChangesDoc }): JSX.Element {
  const muted = 'var(--thread-card-muted, var(--color-text-muted))'
  const more = changes.total - changes.files.length
  return (
    <div data-testid="changes-card" style={{ marginTop: 2, minWidth: 0, fontSize: '0.92em' }}>
      <div data-changes-headline="" style={{ fontWeight: 600 }}>
        {changesHeadline(changes)}
      </div>
      {changes.branch && (
        <div data-changes-branch="" style={{ color: muted, fontSize: '0.86em' }}>
          on {changes.branch}
          {changes.from && changes.to && changes.from !== changes.to ? ` · ${changes.from}..${changes.to}` : ''}
        </div>
      )}
      {changes.files.length > 0 && (
        <ul data-changes-files="" style={{ listStyle: 'none', margin: '6px 0 0', padding: 0, display: 'grid', gap: 2 }}>
          {changes.files.map((f) => {
            const letter = changeLetter(f.status)
            return (
              <li
                key={f.file}
                data-changes-file={f.file}
                style={{ display: 'flex', gap: 8, alignItems: 'baseline', minWidth: 0, fontFamily: 'var(--font-mono, ui-monospace, monospace)', fontSize: '0.86em' }}
              >
                <span title={f.status} style={{ color: STATUS_COLOR[letter], fontWeight: 700, width: '1ch', flex: 'none' }}>
                  {letter}
                </span>
                <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }} title={f.file}>
                  {f.file}
                </span>
                {(f.additions !== null || f.deletions !== null) && (
                  <span style={{ flex: 'none', color: muted }}>
                    {f.additions !== null && <span style={{ color: STATUS_COLOR.A }}>+{f.additions}</span>}{' '}
                    {f.deletions !== null && <span style={{ color: STATUS_COLOR.D }}>{'−'}{f.deletions}</span>}
                  </span>
                )}
              </li>
            )
          })}
        </ul>
      )}
      {more > 0 && (
        <div data-changes-more="" style={{ color: muted, fontSize: '0.86em', marginTop: 4 }}>
          and {more} more file{more === 1 ? '' : 's'}
        </div>
      )}
    </div>
  )
}
