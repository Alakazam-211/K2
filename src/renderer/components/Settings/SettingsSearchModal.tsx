import React, { useEffect, useMemo, useRef, useState } from 'react'
import { floatMaterial } from '@/components/ui/float-material'
import { BetaTag } from '@/components/ui/Tag'
import { DialogCloseButton, useDialogCancel } from '@/components/ui'
import { SearchField } from '@/components/ui/SearchField'
import { searchManifest, sectionLabel, type SettingEntry } from './searchManifest'

interface SettingsSearchModalProps {
  entries: SettingEntry[]
  onPick: (entry: SettingEntry) => void
  onClose: () => void
  /** This client shows Settings → Gardens: Local Data and External Data
   *  results are labelled (and land on) Gardens. */
  gardensPage?: boolean
}

/**
 * Command-palette-style search modal for Settings. Opened from the
 * magnifier button in the Settings sidebar header or the search hotkey.
 * Fuzzy-matches against label, description, keywords, and group; results
 * are grouped visually by section in the dropdown.
 *
 * Keyboard: up/down to navigate, enter to pick, escape to close.
 */
export function SettingsSearchModal({ entries, onPick, onClose, gardensPage = false }: SettingsSearchModalProps): React.JSX.Element {
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

  // Esc (wherever focus is), the × and a press outside close it. The shared
  // listener claims the Escape (preventDefault + stopImmediatePropagation),
  // so the Settings router's Escape-closes-settings doesn't also fire.
  const cancel = useDialogCancel(onClose)

  // Keyboard handling — bound at the container so arrow keys work even
  // when focus is on the input.
  const onKeyDown = (e: React.KeyboardEvent): void => {
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

  // Keep the selected row in view
  useEffect(() => {
    const row = listRef.current?.querySelector<HTMLElement>(`[data-result-index="${selectedIndex}"]`)
    row?.scrollIntoView({ block: 'nearest' })
  }, [selectedIndex])

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center bg-[var(--color-scrim)] pt-[10vh]"
      onKeyDown={onKeyDown}
      tabIndex={-1}
    >
      <div {...floatMaterial('dialog')} {...cancel} className="w-full max-w-xl mx-4 bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-2xl flex flex-col max-h-[70vh]">
        {/* Input */}
        {/* Escape closes the palette here, so it doesn't clear the text. */}
        <div className="flex items-center gap-2 px-3 py-2.5 border-b border-[var(--color-border)] flex-shrink-0">
          <SearchField
            ref={inputRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            escapeClears={false}
            placeholder="Search settings, like font or theme"
            aria-label="Search settings"
            className="flex-1 min-w-0"
          />
          <kbd className="type-hint font-mono text-[var(--color-text-muted)] border border-[var(--color-border)] rounded-[var(--radius-selector)] px-1.5 py-0.5">
            esc
          </kbd>
          <DialogCloseButton className="-mr-1" onClick={onClose} />
        </div>

        {/* Results */}
        <div ref={listRef} className="flex-1 overflow-y-auto py-1">
          {results.length === 0 ? (
            <div className="px-3 py-6 text-center type-hint text-[var(--color-text-muted)]">
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
                  className={`w-full flex items-baseline gap-3 px-3 py-1.5 text-left type-control no-drag cursor-pointer ${
                    isSelected
                      ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.03]'
                  }`}
                >
                  <span className="type-section text-[var(--color-text-muted)] uppercase tracking-wider w-28 flex-shrink-0 truncate">
                    {sectionLabel(entry.section, { gardensPage })}
                    {entry.group && <span className="mx-1 opacity-60">/</span>}
                    {entry.group && <span className="normal-case">{entry.group}</span>}
                  </span>
                  <span className="flex-shrink-0 min-w-0 truncate font-medium">
                    {entry.label}
                  </span>
                  {entry.beta ? <BetaTag /> : null}
                  {entry.description && (
                    <span className="flex-1 min-w-0 truncate type-hint text-[var(--color-text-muted)]">
                      — {entry.description}
                    </span>
                  )}
                </button>
              )
            })
          )}
        </div>

        {/* Footer hint */}
        <div className="flex items-center gap-3 px-3 py-1.5 border-t border-[var(--color-border)] flex-shrink-0 type-hint text-[var(--color-text-muted)]">
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
