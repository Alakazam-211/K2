// prd-projects-v1 §6.5 (relocation directive) — the top-bar settings
// cog, visible on ALL pages immediately AFTER the page tabs (canonical
// anatomy: K2 | Server | Tabs | ⚙ | …). One shared button so the
// workspace TopBar and the Feedback/Projects overlay top bars open
// Settings identically: `useSettingsStore.openSettings()` flips
// `settingsOpen`, and App.tsx's Settings branch replaces the whole
// layout (the overlay pages simply don't render while it's open — the
// page-view store is untouched, so closing Settings lands back on the
// page the user came from).

import { useSettingsStore } from '@/stores/settings'

export default function SettingsGearButton(): React.JSX.Element {
  return (
    <button
      onClick={() => useSettingsStore.getState().openSettings()}
      className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
      style={{
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'no-drag'
      }}
      title="Settings (⌘,)"
    >
      <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
      </svg>
    </button>
  )
}
