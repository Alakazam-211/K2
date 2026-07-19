// Wiki K2 map — custom focus-group filter dropdown.
//
// Same ergonomics as Feedback's WorkspaceFilterDropdown: compact trigger
// sized like a select, popover with search + keyboard nav, square corners,
// dark K2 tokens, capture-phase Esc so page Esc doesn't also close Wiki.

import React, { useEffect, useMemo, useRef, useState } from 'react'
import type { WikiFocusGroup } from './wiki-api'

export type FocusGroupFilterOption = {
  id: string
  name: string
  color?: string | null
  /** Optional count shown on the right (e.g. member workspaces with wikis). */
  count?: number
}

interface FocusGroupFilterDropdownProps {
  groups: WikiFocusGroup[]
  /** `all` | `ungrouped` | focus group id. */
  value: string
  onChange: (value: string) => void
}

export function FocusGroupFilterDropdown({
  groups,
  value,
  onChange,
}: FocusGroupFilterDropdownProps): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const [keyboardIndex, setKeyboardIndex] = useState(-1)
  const rootRef = useRef<HTMLDivElement>(null)
  const searchRef = useRef<HTMLInputElement>(null)

  const options = useMemo((): FocusGroupFilterOption[] => {
    const q = query.trim().toLowerCase()
    const rows: FocusGroupFilterOption[] = [
      { id: 'all', name: 'All' },
      { id: 'ungrouped', name: 'Ungrouped' },
      ...groups.map((g) => ({
        id: g.id,
        name: g.name,
        color: g.color ?? null,
        count: g.workspaceIds.length,
      })),
    ]
    if (!q) return rows
    return rows.filter((r) => r.name.toLowerCase().includes(q))
  }, [groups, query])

  const flatValues = useMemo(() => options.map((o) => o.id), [options])

  const selected =
    value === 'all'
      ? { id: 'all', name: 'All', color: null as string | null }
      : value === 'ungrouped'
        ? { id: 'ungrouped', name: 'Ungrouped', color: null as string | null }
        : groups.find((g) => g.id === value) ?? { id: value, name: 'Focus group', color: null }

  useEffect(() => {
    if (open) {
      requestAnimationFrame(() => searchRef.current?.focus())
    } else {
      setQuery('')
      setKeyboardIndex(-1)
    }
  }, [open])

  useEffect(() => setKeyboardIndex(-1), [query])

  // Outside click closes; capture-phase Esc closes the popover before
  // the wiki page Esc handler (clear selection / close page).
  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent): void => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpen(false)
    }
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.stopPropagation()
        setOpen(false)
      }
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey, true)
    }
  }, [open])

  useEffect(() => {
    if (keyboardIndex < 0) return
    const val = flatValues[keyboardIndex]
    if (!val) return
    const el = rootRef.current?.querySelector(`[data-fg-filter-value="${CSS.escape(val)}"]`)
    el?.scrollIntoView({ block: 'nearest' })
  }, [keyboardIndex, flatValues])

  const pick = (v: string): void => {
    onChange(v)
    setOpen(false)
  }

  const onSearchKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.min(prev + 1, flatValues.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setKeyboardIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter' && keyboardIndex >= 0 && keyboardIndex < flatValues.length) {
      e.preventDefault()
      pick(flatValues[keyboardIndex])
    } else if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      setOpen(false)
    }
  }

  const rowClass = (isSelected: boolean, isKeyboard: boolean): string =>
    `flex items-center gap-2 px-2 py-1.5 cursor-pointer transition-colors w-full text-left ${
      isSelected
        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
        : isKeyboard
          ? 'bg-white/[0.06] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)]'
    }`

  return (
    <div ref={rootRef} className="relative flex-shrink-0 no-drag">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="flex items-center gap-1.5 px-2 py-1.5 text-[11px] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)] outline-none cursor-pointer hover:text-[var(--color-text-primary)] transition-colors max-w-[180px]"
        title="Filter the K2 map by focus group"
      >
        {selected.color && (
          <span
            className="w-2 h-2 flex-shrink-0"
            style={{ backgroundColor: selected.color }}
          />
        )}
        <span className="truncate">{selected.name}</span>
        <svg
          className={`w-2.5 h-2.5 flex-shrink-0 text-[var(--color-text-muted)] transition-transform ${open ? 'rotate-180' : ''}`}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {open && (
        <div className="absolute right-0 top-full mt-1 w-64 z-30 bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-lg flex flex-col">
          <div className="p-1.5 border-b border-[var(--color-border)]">
            <input
              ref={searchRef}
              type="text"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              onKeyDown={onSearchKeyDown}
              placeholder="Search focus groups…"
              className="w-full px-2 py-1.5 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none focus:border-[var(--color-accent)]"
            />
          </div>

          <div className="max-h-72 overflow-y-auto py-1">
            {options.length === 0 ? (
              <div className="px-2 py-4 text-center text-[10px] text-[var(--color-text-muted)]">
                No focus groups match
              </div>
            ) : (
              options.map((opt) => {
                const kbIdx = flatValues.indexOf(opt.id)
                const isSpecial = opt.id === 'all' || opt.id === 'ungrouped'
                return (
                  <button
                    key={opt.id}
                    type="button"
                    data-fg-filter-value={opt.id}
                    onClick={() => pick(opt.id)}
                    className={rowClass(value === opt.id, kbIdx >= 0 && kbIdx === keyboardIndex)}
                  >
                    {opt.color && (
                      <span
                        className="w-1 h-3 flex-shrink-0"
                        style={{ backgroundColor: opt.color }}
                      />
                    )}
                    <span className="text-xs truncate flex-1">{opt.name}</span>
                    {typeof opt.count === 'number' && !isSpecial && (
                      <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums flex-shrink-0">
                        {opt.count}
                      </span>
                    )}
                  </button>
                )
              })
            )}
          </div>
        </div>
      )}
    </div>
  )
}
