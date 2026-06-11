import React from 'react'
import { useEffect, useRef, useState } from 'react'

export function SettingRow({
  label,
  children,
  settingId,
}: {
  label: React.ReactNode
  children: React.ReactNode
  /**
   * Optional stable identifier used by the Settings search palette to
   * scroll this row into view and highlight it. Conventionally matches
   * a `SettingEntry.id` — e.g. `"terminal.font-family"`.
   */
  settingId?: string
}): React.JSX.Element {
  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id={settingId}
    >
      <span className="text-xs text-[var(--color-text-secondary)]">{label}</span>
      {children}
    </div>
  )
}

export function SettingsGroup({
  title,
  badge,
  children
}: {
  title: string
  /** Optional inline badge rendered next to the title — used for
   *  `beta` / status tags. Stays opt-in so existing callers
   *  (Workspace, Worktrees, Chat Migrations) render unchanged. */
  badge?: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <div className="space-y-2">
      <h3 className="text-[10px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider flex items-center gap-2">
        <span>{title}</span>
        {badge}
      </h3>
      <div className="ml-2 pl-3 border-l-2 border-[var(--color-border)] space-y-1">
        {children}
      </div>
    </div>
  )
}

export function SettingDropdown({
  value,
  options,
  onChange,
  className,
}: {
  value: string
  options: { value: string; label: string; disabled?: boolean }[]
  onChange: (value: string) => void | Promise<void>
  className?: string
}): React.JSX.Element {
  const [isOpen, setIsOpen] = useState(false)
  const containerRef = useRef<HTMLDivElement>(null)

  const selected = options.find((o) => o.value === value) ?? options[0]

  useEffect(() => {
    if (!isOpen) return
    const handler = (e: MouseEvent): void => {
      if (containerRef.current && !containerRef.current.contains(e.target as Node)) {
        setIsOpen(false)
      }
    }
    document.addEventListener('mousedown', handler)
    return () => document.removeEventListener('mousedown', handler)
  }, [isOpen])

  return (
    <div ref={containerRef} className={`relative no-drag ${className ?? ''}`}>
      <button
        onClick={() => setIsOpen(!isOpen)}
        className="flex items-center gap-2 px-2 py-1 text-xs bg-[var(--color-bg)] border border-[var(--color-border)] hover:border-[var(--color-text-muted)] text-[var(--color-text-primary)] transition-colors cursor-pointer"
      >
        <span className="truncate">{selected?.label ?? ''}</span>
        <svg
          className={`w-3 h-3 text-[var(--color-text-muted)] flex-shrink-0 transition-transform ${isOpen ? 'rotate-180' : ''}`}
          fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {isOpen && (
        <div className="absolute top-full right-0 z-50 mt-0.5 min-w-full bg-[var(--color-bg-surface)] border border-[var(--color-border)] shadow-xl max-h-60 overflow-y-auto">
          {options.map((option) => {
            const isActive = option.value === value
            const isDisabled = option.disabled === true
            return (
              <button
                key={option.value}
                disabled={isDisabled}
                onClick={() => {
                  if (isDisabled) return
                  onChange(option.value)
                  setIsOpen(false)
                }}
                className={`w-full flex items-center gap-2 px-3 py-1.5 text-left text-xs transition-colors ${
                  isDisabled
                    ? 'text-[var(--color-text-muted)] opacity-50 cursor-not-allowed'
                    : isActive
                      ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/10 cursor-pointer'
                      : 'text-[var(--color-text-secondary)] hover:bg-white/[0.04] hover:text-[var(--color-text-primary)] cursor-pointer'
                }`}
              >
                <span className="truncate flex-1">{option.label}</span>
                {isActive && (
                  <svg className="w-3 h-3 flex-shrink-0 text-[var(--color-accent)]" fill="none" viewBox="0 0 24 24" stroke="currentColor" strokeWidth={2.5}>
                    <path strokeLinecap="round" strokeLinejoin="round" d="M5 13l4 4L19 7" />
                  </svg>
                )}
              </button>
            )
          })}
        </div>
      )}
    </div>
  )
}
