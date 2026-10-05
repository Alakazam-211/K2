// Omarchy additions 2 and 4 (prd-zen-mode-v1, Rosson 2026-10-04) — the
// in-Zen theme picker and the Zen shortcut cheat sheet, both drawn by K2
// with `--zen-*` tokens only (never Styles tokens).
//
// - Theme picker: a small swatch button in the template's top-right
//   cluster, immediately left of the Zen toggle (Rosson 2026-10-04; the
//   template places it through `ZenK2TopRightContext`). It lists
//   the daemon's themes (built-in ones marked), checks the active one, and
//   switches on click. ⌃⌘. / ⌃⌘⇧. (Ctrl+Alt+. / Ctrl+Alt+Shift+.) cycle.
// - Cheat sheet: `?` (not while typing), ⌃⌘/ (Ctrl+Alt+/), the macOS View
//   menu's "Zen Shortcuts", or the Linux / Windows app menu's. Esc closes.
// Both are registered as K2 overlays, so the required-controls check never
// counts them as covering the page's controls.

import { useCallback, useEffect, useRef, useState } from 'react'
import type { DesktopOs } from '@/lib/desktop-chrome'
import { registerZenK2Overlay } from '@/lib/zen/zen-controls'
import { zenThemeLabel, type ZenThemeEntry } from '@/lib/zen/zen-page'
import {
  closeZenOverlays,
  useZenOverlayStore,
  zenShortcutGroups,
} from '@/lib/zen/zen-theme-switch'

function useK2Overlay(): (el: HTMLElement | null) => void {
  const off = useRef<(() => void) | null>(null)
  useEffect(() => () => off.current?.(), [])
  return useCallback((el: HTMLElement | null) => {
    off.current?.()
    off.current = el ? registerZenK2Overlay(el) : null
  }, [])
}

function useEscCloses(open: boolean): void {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      closeZenOverlays()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open])
}

export function ZenThemePicker({
  themes,
  active,
  onPick,
  error,
}: {
  themes: readonly ZenThemeEntry[]
  active: string | null
  onPick(name: string): void
  error: string | null
}): React.JSX.Element | null {
  const open = useZenOverlayStore((s) => s.picker)
  const overlayRef = useK2Overlay()
  useEscCloses(open)
  if (themes.length === 0) return null
  return (
    <div
      ref={overlayRef}
      data-zen-theme-picker=""
      className="no-drag"
      style={{
        // In the template's top-right cluster, left of the Zen toggle; the
        // list opens below, right-aligned, over the page.
        position: 'relative',
        flexShrink: 0,
        zIndex: 20,
      }}
    >
      {open && (
        <div
          role="listbox"
          aria-label="Zen themes"
          style={{
            position: 'absolute',
            right: 0,
            top: 'calc(100% + 6px)',
            minWidth: 200,
            maxHeight: 320,
            overflowY: 'auto',
            padding: 4,
            background: 'var(--zen-surface-raised)',
            color: 'var(--zen-text)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'var(--zen-radius)',
            boxShadow: '0 8px 24px rgba(0, 0, 0, 0.18)',
          }}
        >
          {themes.map((t) => {
            const isActive = t.name === active
            return (
              <button
                className="cursor-pointer"
                key={t.name}
                type="button"
                role="option"
                aria-selected={isActive}
                data-zen-theme-option={t.name}
                onClick={() => onPick(t.name)}
                style={{
                  display: 'flex',
                  width: '100%',
                  alignItems: 'center',
                  gap: 8,
                  padding: '6px 8px',
                  border: 0,
                  borderRadius: 'calc(var(--zen-radius) - 4px)',
                  background: isActive ? 'var(--zen-surface)' : 'transparent',
                  color: 'inherit',
                  font: 'inherit',
                  textAlign: 'left',
                  cursor: 'pointer',
                }}
              >
                <span aria-hidden style={{ width: 12, color: 'var(--zen-accent)' }}>
                  {isActive ? '✓' : ''}
                </span>
                <span style={{ flex: 1 }}>{t.label}</span>
                {t.builtin && <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>built in</span>}
              </button>
            )
          })}
          {error && (
            <div role="alert" data-zen-theme-error="" style={{ padding: '6px 8px', color: 'var(--zen-danger)' }}>
              {error}
            </div>
          )}
        </div>
      )}
      <button
        className="cursor-pointer"
        type="button"
        aria-label={active ? `Theme: ${active}` : 'Themes'}
        aria-haspopup="listbox"
        aria-expanded={open}
        title={active ? `Theme: ${active}` : 'Themes'}
        data-zen-theme-button=""
        onClick={() => useZenOverlayStore.setState({ picker: !open, sheet: false })}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 6,
          height: 26,
          padding: '0 9px',
          borderRadius: 13,
          border: '1px solid var(--zen-border)',
          background: 'var(--zen-surface)',
          color: 'var(--zen-text-muted)',
          fontSize: 12,
          cursor: 'pointer',
          opacity: open ? 1 : 0.75,
        }}
      >
        <span
          aria-hidden
          style={{
            width: 10,
            height: 10,
            borderRadius: 5,
            background: 'linear-gradient(135deg, var(--zen-accent) 50%, var(--zen-bubble-agent) 50%)',
          }}
        />
        <span data-zen-active-theme="">
          {active ? (themes.find((t) => t.name === active)?.label ?? zenThemeLabel(active)) : 'Theme'}
        </span>
      </button>
    </div>
  )
}

export function ZenShortcutSheet({ os }: { os: DesktopOs }): React.JSX.Element | null {
  const open = useZenOverlayStore((s) => s.sheet)
  const overlayRef = useK2Overlay()
  useEscCloses(open)
  const [groups] = useState(() => zenShortcutGroups(os))
  if (!open) return null
  return (
    <div
      ref={overlayRef}
      data-zen-shortcut-sheet=""
      className="no-drag"
      onClick={closeZenOverlays}
      style={{
        position: 'absolute',
        inset: 0,
        zIndex: 30,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0, 0, 0, 0.32)',
      }}
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Zen shortcuts"
        onClick={(e) => e.stopPropagation()}
        style={{
          width: 'min(560px, calc(100% - 48px))',
          maxHeight: 'calc(100% - 64px)',
          overflowY: 'auto',
          padding: 20,
          background: 'var(--zen-surface-raised)',
          color: 'var(--zen-text)',
          border: '1px solid var(--zen-border)',
          borderRadius: 'var(--zen-radius)',
          boxShadow: '0 16px 48px rgba(0, 0, 0, 0.24)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'baseline', justifyContent: 'space-between', marginBottom: 12 }}>
          <h2 style={{ margin: 0, fontSize: '1.15em', fontWeight: 600 }}>Zen shortcuts</h2>
          <button
            className="cursor-pointer"
            type="button"
            onClick={closeZenOverlays}
            style={{ border: 0, background: 'transparent', color: 'var(--zen-text-muted)', cursor: 'pointer', font: 'inherit' }}
          >
            Close
          </button>
        </div>
        {groups.map((g) => (
          <section key={g.title} style={{ marginTop: 12 }}>
            <h3
              style={{
                margin: '0 0 6px',
                fontSize: '0.78em',
                fontWeight: 600,
                letterSpacing: '0.06em',
                textTransform: 'uppercase',
                color: 'var(--zen-text-muted)',
              }}
            >
              {g.title}
            </h3>
            {g.rows.map((r) => (
              <div
                key={`${g.title}:${r.keys}`}
                data-zen-shortcut={r.keys}
                style={{ display: 'flex', justifyContent: 'space-between', gap: 16, padding: '4px 0' }}
              >
                <span>{r.what}</span>
                <kbd
                  style={{
                    fontFamily: 'var(--zen-terminal-font-family)',
                    fontSize: '0.9em',
                    padding: '1px 6px',
                    borderRadius: 6,
                    border: '1px solid var(--zen-border)',
                    background: 'var(--zen-surface)',
                    whiteSpace: 'nowrap',
                  }}
                >
                  {r.keys}
                </kbd>
              </div>
            ))}
          </section>
        ))}
      </div>
    </div>
  )
}
