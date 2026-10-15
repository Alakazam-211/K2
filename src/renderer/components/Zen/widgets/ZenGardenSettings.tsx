// The Garden's own settings (0.45.6, Rosson 2026-10-15; ported from K2
// Companion's in-Zen settings sheet, `ZenSettings.tsx` → Lists). Reached
// from inside the Garden: the cog in Garden 1's Agents header (the Agents
// widget's `settings` prop). Never in the app's Settings window, which stays
// as it is.
//
// One group today, Lists: "Recent agents first" (`zen-recent-first.ts`),
// on this computer only, off until you turn it on.

import { useState } from 'react'
import { useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZenOverlay, zenButtonStyle } from './ZenOverlay'

export const ZEN_RECENT_FIRST_LABEL = 'Recent agents first'
export const ZEN_RECENT_FIRST_NOTE =
  'Below your pinned agents, the agents you last messaged from a Garden on this computer come first, on My Home and Agents. Only this computer; the app’s Agents page doesn’t change.'

function Switch({ on, onToggle, labelledBy }: { on: boolean; onToggle(): void; labelledBy: string }): React.JSX.Element {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-labelledby={labelledBy}
      data-zen-recent-first-switch={on ? 'on' : 'off'}
      onClick={onToggle}
      className="relative flex-shrink-0 cursor-pointer"
      style={{
        width: 36,
        height: 20,
        borderRadius: 999,
        border: 'none',
        background: on ? 'var(--zen-accent)' : 'var(--zen-border)',
        transition: 'background-color 150ms',
      }}
    >
      <span
        aria-hidden
        style={{
          position: 'absolute',
          top: 2,
          left: on ? 18 : 2,
          width: 16,
          height: 16,
          borderRadius: 999,
          background: on ? 'var(--zen-accent-text, #fff)' : 'var(--zen-surface-raised)',
          boxShadow: '0 1px 2px rgba(0, 0, 0, 0.25)',
          transition: 'left 150ms',
        }}
      />
    </button>
  )
}

export function ZenGardenSettings({ onClose }: { onClose(): void }): React.JSX.Element {
  const on = useZenRecentFirstStore((s) => s.on)
  const setOn = useZenRecentFirstStore((s) => s.setOn)
  return (
    <ZenOverlay label="Garden settings" onClose={onClose} testId="zen-garden-settings" width={420}>
      <strong style={{ fontSize: '1.1em' }}>Garden settings</strong>
      <section aria-labelledby="zen-garden-settings-lists" data-zen-garden-settings-lists="" className="flex flex-col" style={{ gap: 6 }}>
        <span
          id="zen-garden-settings-lists"
          style={{ fontSize: '0.75em', fontWeight: 600, letterSpacing: '0.04em', textTransform: 'uppercase', color: 'var(--zen-text-muted)' }}
        >
          Lists
        </span>
        <div
          className="flex items-center justify-between"
          style={{ gap: 12, padding: '8px 12px', borderRadius: 'var(--zen-radius)', background: 'var(--zen-surface)', border: '1px solid var(--zen-border)' }}
        >
          <span id="zen-recent-first-label">{ZEN_RECENT_FIRST_LABEL}</span>
          <Switch on={on} onToggle={() => setOn(!on)} labelledBy="zen-recent-first-label" />
        </div>
        <span data-zen-recent-first-note="" style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>
          {ZEN_RECENT_FIRST_NOTE}
        </span>
      </section>
      <div className="flex items-center justify-end">
        <button type="button" data-zen-garden-settings-done="" onClick={onClose} className="cursor-pointer" style={zenButtonStyle(true)}>
          Done
        </button>
      </div>
    </ZenOverlay>
  )
}

/** The cog that opens the Garden's settings (the Agents header). */
export function ZenGardenSettingsButton(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button
        type="button"
        aria-label="Garden settings"
        title="Garden settings"
        aria-haspopup="dialog"
        data-zen-agents-settings=""
        data-zen-soft-button=""
        onClick={() => setOpen(true)}
        className="no-drag flex flex-shrink-0 items-center justify-center cursor-pointer"
        style={{ width: 24, height: 24, borderRadius: 999, color: 'var(--zen-text-muted)' }}
      >
        <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      </button>
      {open && <ZenGardenSettings onClose={() => setOpen(false)} />}
    </>
  )
}
