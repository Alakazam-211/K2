import { useState, useRef, useEffect, useCallback } from 'react'

interface FocusGroupOption {
  id: string | null
  name: string
  color?: string | null
}

interface FocusGroupDropdownProps {
  options: FocusGroupOption[]
  value: string | null
  onChange: (id: string | null) => void
}

export default function FocusGroupDropdown({
  options,
  value,
  onChange
}: FocusGroupDropdownProps): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const [search, setSearch] = useState('')
  const containerRef = useRef<HTMLDivElement>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const [highlightIndex, setHighlightIndex] = useState(0)

  const allOptions: FocusGroupOption[] = options

  const filtered = search
    ? allOptions.filter((o) => o.name.toLowerCase().includes(search.toLowerCase()))
    : allOptions

  const selected = allOptions.find((o) => o.id === value) ?? allOptions[0] ?? { id: null, name: 'Focus Groups' }

  // Close on outside click
  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent): void => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false)
        setSearch('')
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  // Focus input when opening
  useEffect(() => {
    if (isOpen && inputRef.current) {
      inputRef.current.focus()
    }
  }, [isOpen])

  // Reset highlight when search changes
  useEffect(() => {
    setHighlightIndex(0)
  }, [search])

  const handleSelect = useCallback((id: string | null) => {
    onChange(id)
    setIsOpen(false)
    setSearch('')
  }, [onChange])

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      e.preventDefault()
      setIsOpen(false)
      setSearch('')
    } else if (e.key === 'ArrowDown') {
      e.preventDefault()
      setHighlightIndex((prev) => Math.min(prev + 1, filtered.length - 1))
    } else if (e.key === 'ArrowUp') {
      e.preventDefault()
      setHighlightIndex((prev) => Math.max(prev - 1, 0))
    } else if (e.key === 'Enter') {
      e.preventDefault()
      if (filtered[highlightIndex]) {
        handleSelect(filtered[highlightIndex].id)
      }
    }
  }, [filtered, highlightIndex, handleSelect])

  return (
    <div ref={containerRef} className="relative no-drag">
      {/* Trigger button */}
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="w-full flex items-center gap-2 px-2 py-1.5 text-left bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] transition-colors cursor-pointer"
      >
        {selected.color && (
          <span className="w-2 h-2 flex-shrink-0" style={{ backgroundColor: selected.color }} />
        )}
        <span className="text-[11px] font-semibold text-[var(--color-text-primary)] uppercase tracking-wide truncate flex-1">
          {selected.name}
        </span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {/* Dropdown panel */}
      {isOpen && (
        <div className="absolute top-full left-0 right-0 z-50 mt-0.5 bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-xl">
          {/* Search input */}
          <div className="p-1.5 border-b border-[var(--color-border)]">
            <input
              ref={inputRef}
              type="text"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="Search groups..."
              className="w-full px-2 py-1 text-[11px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]"
            />
          </div>

          {/* Options list */}
          <div className="max-h-48 overflow-y-auto py-0.5">
            {filtered.length === 0 ? (
              <div className="px-3 py-2 text-[11px] text-[var(--color-text-muted)] italic">
                No matches
              </div>
            ) : (
              filtered.map((option, i) => {
                const isActive = option.id === value
                const isHighlighted = i === highlightIndex

                return (
                  <button
                    key={option.id ?? '__all__'}
                    onClick={() => handleSelect(option.id)}
                    onMouseEnter={() => setHighlightIndex(i)}
                    className={`w-full flex items-center gap-2 px-3 py-1.5 text-left text-[11px] transition-colors cursor-pointer ${
                      isHighlighted
                        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : isActive
                          ? 'text-[var(--color-accent)]'
                          : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04]'
                    }`}
                  >
                    {option.color ? (
                      <span className="w-2 h-2 flex-shrink-0" style={{ backgroundColor: option.color }} />
                    ) : (
                      <span className="w-2 flex-shrink-0" />
                    )}
                    <span className="truncate flex-1">{option.name}</span>
                    {isActive && (
                      <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                        <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                      </svg>
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
