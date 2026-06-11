import React, { useEffect, useMemo, useRef, useState } from 'react'
import { SECTION_LABELS, searchManifest, type SettingEntry } from './searchManifest'

interface SettingsSearchModalProps {
  entries: SettingEntry[]
  onPick: (entry: SettingEntry) => void
  onClose: () => void
}

/**
 * Command-palette-style search modal for Settings. Opened from the
 * magnifier button in the Settings sidebar header or the search hotkey.
 * Fuzzy-matches against label, description, keywords, and group; results
 * are grouped visually by section in the dropdown.
 *
 * Keyboard: up/down to navigate, enter to pick, escape to close.
 */
export function SettingsSearchModal({ entries, onPick, onClose }: SettingsSearchModalProps): React.JSX.Element {
  const [query, setQuery] = useState('')
  const [selectedIndex, setSelectedIndex] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)

  // Filter + rank in a memo so big manifests don't re-score every render
  const results = useMemo(() => searchManifest(entries, query).slice(0, 50), [entries, query])

  // Clamp selection into range when results shrink
  useEffect(() => {
    setSelectedIndex((i) => Math.min(i, Math.max(0, results.length - 1)))
  }, [results.length])

  // Focus the input on open
  useEffect(() => {
    inputRef.current?.focus()
  }, [])

  // Keyboard handling — bound at the container so arrow keys work even
  // when focus is on the input. We stop propagation so outer handlers
  // (e.g. the Settings router's Escape-closes-settings) don't also
  // fire; React synthetic + native are separate layers and without
  // stopPropagation the Escape bubbles to the window listener.
  const onKeyDown = (e: React.KeyboardEvent): void => {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      ;(e.nativeEvent as KeyboardEvent).stopImmediatePropagation?.()
      onClose()
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      setSelectedIndex((i) => Math.min(i + 1, Math.max(0, results.length - 1)))
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      setSelectedIndex((i) => Math.max(0, i - 1))
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      const chosen = results[selectedIndex]
      if (chosen) onPick(chosen)
      return
    }
  }

  // Belt-and-suspenders: the React synthetic handler above fires before
  // the native bubble reaches `window`, but we also install a native
  // capture-phase listener so a third party with window.addEventListener
  // can't race past us. When the modal is open, any Escape on window
  // is consumed here.
  useEffect(() => {
    const nativeHandler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopImmediatePropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', nativeHandler, true) // capture = true
    return () => window.removeEventListener('keydown', nativeHandler, true)
  }, [onClose])

  // Keep the selected row in view
  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>(`[data-result-index="${selectedIndex}"]`)
    row?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-black/40 pt-[10vh]"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose() }}
      onKeyDown={onKeyDown}
      tabIndex={-1}
    >
      <div className="w-full max-w-xl mx-4 bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-2xl flex flex-col max-h-[70vh]">
        {/* Input */}
        <div className="flex items-center gap-2 px-3 py-2.5 border-b border-[var(--color-border)] flex-shrink-0">
          <svg className="w-3.5 h-3.5 text-[var(--color-text-muted)] flex-shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
            <circle cx="11" cy="11" r="8" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search settings… (try: font, theme, hidden files, clear terminal)"
            className="flex-1 bg-transparent text-xs text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] focus:outline-none"
          />
          <kbd className="text-[9px] font-mono text-[var(--color-text-muted)] border border-[var(--color-border)] px-1.5 py-0.5">
            esc
          </kbd>
        </div>

        {/* Results */}
        <div ref={listRef} className="flex-1 overflow-y-auto py-1">
          {results.length === 0 ? (
            <div className="px-3 py-6 text-center text-xs text-[var(--color-text-muted)]">
              {query.trim() ? `No settings match "${query}"` : 'Type to search…'}
            </div>
          ) : (
            results.map((entry, i) => {
              const isSelected = i === selectedIndex
              return (
                <button
                  key={entry.id}
                  data-result-index={i}
                  onMouseEnter={() => setSelectedIndex(i)}
                  onClick={() => onPick(entry)}
                  className={`w-full flex items-baseline gap-3 px-3 py-1.5 text-left text-xs no-drag cursor-pointer ${
                    isSelected
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.03]'
                  }`}
                >
                  <span className="text-[10px] text-[var(--color-text-muted)] uppercase tracking-wider w-28 flex-shrink-0 truncate">
                    {SECTION_LABELS[entry.section] ?? entry.section}
                    {entry.group && <span className="mx-1 opacity-60">/</span>}
                    {entry.group && <span className="normal-case">{entry.group}</span>}
                  </span>
                  <span className="flex-shrink-0 min-w-0 truncate font-medium">
                    {entry.label}
                  </span>
                  {entry.description && (
                    <span className="flex-1 min-w-0 truncate text-[10px] text-[var(--color-text-muted)]">
                      — {entry.description}
                    </span>
                  )}
                </button>
              )
            })
          )}
        </div>

        {/* Footer hint */}
        <div className="flex items-center gap-3 px-3 py-1.5 border-t border-[var(--color-border)] flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1">
            <kbd className="font-mono border border-[var(--color-border)] px-1">↑</kbd>
            <kbd className="font-mono border border-[var(--color-border)] px-1">↓</kbd>
            navigate
          </span>
          <span className="flex items-center gap-1">
            <kbd className="font-mono border border-[var(--color-border)] px-1">↵</kbd>
            open
          </span>
          <span className="ml-auto tabular-nums">{results.length} result{results.length === 1 ? '' : 's'}</span>
        </div>
      </div>
    </div>
  )
}
