// The standard Wiki view's left column: every note in scope, grouped
// (prd-wiki-3d-view-v1 §12). Search, the workspace picker, All workspaces,
// Global/Local and the K2 lens + filters all come from WikiPage, so the
// list shows the same notes as the 2D and 3D graphs.

import React, { useEffect, useMemo, useRef } from 'react'
import type { WikiIndex } from './wiki-api'
import { isWikiHomeNode } from './wiki-api'
import type { WikiGraphOpts } from './wiki-graph-data'
import { groupWikiNotes, wikiListNotes } from './wiki-standard'

export default function WikiNoteList({
  index,
  opts,
  selectedId,
  onSelect,
}: {
  index: WikiIndex
  opts: WikiGraphOpts
  selectedId: string | null
  onSelect: (id: string) => void
}): React.JSX.Element {
  const groups = useMemo(() => {
    const notes = wikiListNotes(index, opts)
    return groupWikiNotes(notes, opts.mode === 'k2')
  }, [index, opts])
  const total = groups.reduce((n, g) => n + g.notes.length, 0)
  const listRef = useRef<HTMLDivElement>(null)

  // Keep the open note in view when it changes from elsewhere (a graph
  // click, a wikilink, a backlink).
  useEffect(() => {
    if (!selectedId) return
    const rows = listRef.current?.querySelectorAll<HTMLElement>('[data-wiki-note-id]') ?? []
    const el = Array.from(rows).find((r) => r.getAttribute('data-wiki-note-id') === selectedId)
    if (el && typeof el.scrollIntoView === 'function') {
      el.scrollIntoView({ block: 'nearest' })
    }
  }, [selectedId])

  return (
    <nav
      aria-label="Wiki notes"
      data-wiki-note-list
      className="flex flex-col min-h-0 h-full bg-[var(--color-bg)]"
    >
      <div ref={listRef} className="flex-1 min-h-0 overflow-auto py-1.5">
        {total === 0 && (
          <p className="px-3 py-4 text-[11px] text-[var(--color-text-muted)]">
            {opts.search.trim() ? 'No notes match this search.' : 'No notes in this view.'}
          </p>
        )}
        {groups.map((g) => (
          <section key={g.key} data-wiki-note-group={g.key} aria-label={g.label ?? undefined} className="mb-1">
            {g.label && (
              <h3 className="px-3 pt-2 pb-1 text-[9px] uppercase tracking-wide text-[var(--color-text-muted)] select-none truncate">
                {g.label}
                <span className="ml-1 tabular-nums opacity-70">{g.notes.length}</span>
              </h3>
            )}
            <ul>
              {g.notes.map((n) => {
                const on = n.id === selectedId
                return (
                  <li key={n.id}>
                    <button
                      type="button"
                      data-wiki-note-id={n.id}
                      aria-current={on ? 'true' : undefined}
                      onClick={() => onSelect(n.id)}
                      title={n.title || n.id}
                      className={`w-full text-left px-3 py-1 text-[11px] truncate transition-colors cursor-pointer ${
                        on
                          ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.04]'
                      } ${isWikiHomeNode(n) ? 'font-medium' : ''}`}
                    >
                      {n.title || n.id}
                    </button>
                  </li>
                )
              })}
            </ul>
          </section>
        ))}
      </div>
    </nav>
  )
}
