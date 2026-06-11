import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { SettingsSection } from '@/stores/settings'
import { SectionErrorBoundary } from './SectionErrorBoundary'
import { SettingsSearchModal } from './SettingsSearchModal'
import type { SettingEntry } from './searchManifest'
import { GeneralSection, GeneralRemoteHostPanel, GENERAL_MANIFEST } from './sections/GeneralSection'
import { useConnectHostStore } from '@/stores/connect-host'
import { TerminalSection, TERMINAL_MANIFEST } from './sections/TerminalSection'
import { CodeEditorSettingsSection, CODE_EDITOR_MANIFEST } from './sections/CodeEditorSettingsSection'
import { EditorsAgentsSection, EDITORS_AGENTS_MANIFEST } from './sections/EditorsAgentsSection'
import { KeybindingsSection, KEYBINDINGS_MANIFEST } from './sections/KeybindingsSection'
import { TimerSection, TIMER_MANIFEST } from './sections/TimerSection'
import { CompanionSection, COMPANION_MANIFEST } from './sections/CompanionSection'
import { ConnectionsSection, CONNECTIONS_MANIFEST } from './sections/ConnectionsSection'
import { K2ConnectSection, K2_CONNECT_MANIFEST } from './sections/K2ConnectSection'
import { ProjectsSection, PROJECTS_MANIFEST } from './sections/ProjectsSection'
import { WorkspaceStatesSection, WORKSPACE_STATES_MANIFEST } from './sections/WorkspaceStatesSection'
import { AgentSkillsSection, AGENT_SKILLS_MANIFEST } from './sections/AgentSkillsSection'
// HeartbeatsPanel is rendered inline inside ProjectsSection now; manifest
// stays exported from HeartbeatsSection so searches still find it.
import { HEARTBEATS_MANIFEST } from './sections/HeartbeatsSection'
import { WakeSchedulerSection, WAKE_SCHEDULER_MANIFEST } from './sections/WakeSchedulerSection'
import { PermissionsSection, PERMISSIONS_MANIFEST } from './sections/PermissionsSection'
import { DictationLabSection, DICTATION_LAB_MANIFEST } from './sections/DictationLabSection'
import ServerSwitcher from '../TopBar/ServerSwitcher'
import { TOPBAR_HEIGHT } from '../../../shared/constants'

// ── Section nav items ────────────────────────────────────────────────
const SECTIONS: { id: SettingsSection; label: string; agenticOnly?: boolean }[] = [
  { id: 'general', label: 'General' },
  { id: 'projects', label: 'Workspaces' },
  { id: 'workspace-states', label: 'Workspace States', agenticOnly: true },
  { id: 'agent-skills', label: 'Canonical Agent Flow', agenticOnly: true },
  { id: 'k2-connect', label: 'K2 Toge' },
  { id: 'companion', label: 'K2 Companion' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'code-editor', label: 'Code Editor' },
  { id: 'editors-agents', label: 'Editors & Agents' },
  { id: 'wake-scheduler', label: 'Heartbeats', agenticOnly: true },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'timer', label: 'Timer' },
  { id: 'permissions', label: 'Permissions' },
  // 0.37.9 — DEV-only Dictation Lab. Filtered out at render time
  // when `import.meta.env.DEV` is false so production users never
  // see it. Lets us isolate which input config makes Apple
  // Dictation engage cleanly vs. hang.
  ...(import.meta.env.DEV
    ? ([{ id: 'dictation-lab', label: 'Dictation Lab (dev)' }] as const)
    : []),
]

// ── Main Settings component ──────────────────────────────────────────
// This component is a router only — each section lives in its own file
// under ./sections/. Navigation callers (update toasts, "jump to
// settings" buttons, workspace-relation shortcuts) use
// `useSettingsStore.setState({ activeSection: '<id>' })` — the section
// IDs here are the stable contract.
export default function Settings(): React.JSX.Element {
  const activeSection = useSettingsStore((s) => s.activeSection)
  const setSection = useSettingsStore((s) => s.setSection)
  const closeSettings = useSettingsStore((s) => s.closeSettings)
  // General splits into a half/half pane (host controls on the right, full-
  // height divider) ONLY when connected to a remote host — same shell idiom
  // as k2-connect/connections below.
  const isRemote = useConnectHostStore((s) => s.activeHost) !== 'local'
  const [searchOpen, setSearchOpen] = useState(false)

  // Flat manifest across every section — filtered by agenticSystemsEnabled
  // so users can't jump to a gated section they don't have enabled.
  const agenticEnabled = useSettingsStore((s) => s.agenticSystemsEnabled)
  const allEntries = useMemo<SettingEntry[]>(() => {
    const combined: SettingEntry[] = [
      ...GENERAL_MANIFEST,
      ...PROJECTS_MANIFEST,
      ...WORKSPACE_STATES_MANIFEST,
      ...AGENT_SKILLS_MANIFEST,
      ...HEARTBEATS_MANIFEST,
      ...TERMINAL_MANIFEST,
      ...CODE_EDITOR_MANIFEST,
      ...EDITORS_AGENTS_MANIFEST,
      ...KEYBINDINGS_MANIFEST,
      ...TIMER_MANIFEST,
      ...COMPANION_MANIFEST,
      ...CONNECTIONS_MANIFEST,
      ...K2_CONNECT_MANIFEST,
      ...WAKE_SCHEDULER_MANIFEST,
      ...PERMISSIONS_MANIFEST,
      ...DICTATION_LAB_MANIFEST,
    ]
    if (agenticEnabled) return combined
    return combined.filter(
      (e) =>
        e.section !== 'workspace-states' &&
        e.section !== 'agent-skills' &&
        e.section !== 'heartbeats' &&
        e.section !== 'wake-scheduler'
    )
  }, [agenticEnabled])

  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape' && !searchOpen) {
        e.preventDefault()
        closeSettings()
        return
      }
      // CMD/CTRL+F opens the search palette from anywhere inside Settings.
      // Using capture so editor/input fields don't swallow it first.
      if ((e.metaKey || e.ctrlKey) && e.key === 'f' && !searchOpen) {
        e.preventDefault()
        setSearchOpen(true)
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [closeSettings, searchOpen])

  // Jump to a picked entry: switch section, then next frame scroll the
  // matching `data-settings-id` row into view and pulse-highlight it
  // so the user's eye lands on the right control.
  const handlePick = useCallback((entry: SettingEntry) => {
    setSearchOpen(false)
    setSection(entry.section)
    // Defer so the section mounts, renders, and gets a chance to lay out
    // before we query for the row. Two rAFs is usually enough even for
    // heavier sections (Projects, Code Editor).
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = document.querySelector<HTMLElement>(`[data-settings-id="${cssEscape(entry.id)}"]`)
        if (!el) return
        el.scrollIntoView({ behavior: 'smooth', block: 'center' })
        el.classList.add('settings-search-pulse')
        window.setTimeout(() => {
          el.classList.remove('settings-search-pulse')
        }, 1500)
      })
    })
  }, [setSection])

  return (
    <div className="flex flex-col h-full w-full min-h-0 bg-[var(--color-bg)]">
      {/* Top-bar — mirrors the main page's top-bar (TopBar.tsx left cluster):
          traffic-light spacer + "K2" wordmark + ServerSwitcher, so Settings
          shows "K2 <Server Name>" up top exactly like the main view. The
          active-server display/switcher lives HERE now (relocated out of the
          settings sidebar) so the connected-host context is always visible
          and switchable while editing that host's settings. */}
      <div
        className="flex items-center border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none flex-shrink-0"
        data-tauri-drag-region
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2">
          {/* Traffic lights occupy ~70px on macOS */}
          <div style={{ width: 70 }} />
          {/* App name (in-app wordmark) */}
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          {/* K2 server switcher (Local / saved servers / add) */}
          <ServerSwitcher />
        </div>
      </div>

      <div className="flex flex-1 w-full min-h-0">
      {/* Left nav */}
      <div className="w-48 flex-shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-surface)] flex flex-col min-h-0">
        <div className="flex items-center justify-between px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
          <span className="text-xs font-medium text-[var(--color-text-secondary)] uppercase tracking-wider">
            Settings
          </span>
          <button
            onClick={() => setSearchOpen(true)}
            className="flex items-center justify-center w-5 h-5 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer no-drag"
            title="Search settings (⌘F)"
          >
            <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round">
              <circle cx="11" cy="11" r="8" />
              <line x1="21" y1="21" x2="16.65" y2="16.65" />
            </svg>
          </button>
        </div>
        <nav className="flex-1 py-1 overflow-y-auto">
          {SECTIONS.filter((s) => !s.agenticOnly || useSettingsStore.getState().agenticSystemsEnabled).map((s) => (
            <button
              key={s.id}
              onClick={() => setSection(s.id)}
              className={`w-full text-left px-4 py-1.5 text-xs no-drag cursor-pointer transition-colors ${
                activeSection === s.id
                  ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
              }`}
            >
              {s.label}
            </button>
          ))}
        </nav>
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex-shrink-0">
          <button
            onClick={closeSettings}
            className="flex items-center gap-2 text-xs text-[var(--color-text-primary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          >
            &larr; Back
            <span className="text-[10px] text-[var(--color-text-muted)]">Esc</span>
          </button>
        </div>
      </div>

      {/* Content area */}
      <div
        className={`flex-1 min-h-0 relative ${
          activeSection === 'general' ||
          activeSection === 'projects' ||
          activeSection === 'k2-connect' ||
          activeSection === 'connections'
            ? 'overflow-hidden p-0'
            : activeSection === 'dictation-lab'
              ? 'overflow-hidden p-6'
              : 'overflow-y-auto p-6'
        }`}
      >
        {activeSection === 'general' && (
          /* General always sits in the LEFT half (half-width). The RIGHT half
             carries the host-only controls + a full-height divider ONLY when
             connected to a remote host; when local it's an empty pane with no
             divider (General just stays half-width). Mirrors the k2-connect /
             connections half/half shell layout below. */
          <div className="flex h-full min-h-0">
            <div className="flex-1 min-w-0 overflow-y-auto p-6">
              <GeneralSection />
            </div>
            <div
              className={`flex-1 min-w-0 overflow-y-auto p-6 ${
                isRemote ? 'border-l border-[var(--color-border)]' : ''
              }`}
            >
              {isRemote && (
                <SectionErrorBoundary>
                  <GeneralRemoteHostPanel />
                </SectionErrorBoundary>
              )}
            </div>
          </div>
        )}
        {activeSection === 'terminal' && <TerminalSection />}
        {activeSection === 'code-editor' && <CodeEditorSettingsSection />}
        {activeSection === 'editors-agents' && <EditorsAgentsSection />}
        {activeSection === 'keybindings' && <KeybindingsSection />}
        {activeSection === 'timer' && (
          <SectionErrorBoundary>
            <TimerSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'companion' && (
          <SectionErrorBoundary>
            <CompanionSection />
          </SectionErrorBoundary>
        )}
        {(activeSection === 'k2-connect' || activeSection === 'connections') && (
          <div className="flex h-full min-h-0">
            {/* Left pane: K2 Connect (host/expose). Scrolls independently. */}
            <div className="flex-1 min-w-0 overflow-y-auto p-6">
              <SectionErrorBoundary>
                <K2ConnectSection />
              </SectionErrorBoundary>
            </div>
            {/* Right pane: Connections (servers you connect to). The
                border-l is a full-height divider because both flex panes
                stretch to the container's h-full (mirrors the Workspaces
                settings layout). */}
            <div className="flex-1 min-w-0 overflow-y-auto p-6 border-l border-[var(--color-border)]">
              <SectionErrorBoundary>
                <ConnectionsSection />
              </SectionErrorBoundary>
            </div>
          </div>
        )}
        {activeSection === 'projects' && (
          <SectionErrorBoundary>
            <ProjectsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'workspace-states' && (
          <SectionErrorBoundary>
            <WorkspaceStatesSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'agent-skills' && (
          <SectionErrorBoundary>
            <AgentSkillsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'wake-scheduler' && (
          <SectionErrorBoundary>
            <WakeSchedulerSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'permissions' && (
          <SectionErrorBoundary>
            <PermissionsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'dictation-lab' && import.meta.env.DEV && (
          <SectionErrorBoundary>
            <DictationLabSection />
          </SectionErrorBoundary>
        )}
      </div>
      </div>

      {searchOpen && (
        <SettingsSearchModal
          entries={allEntries}
          onPick={handlePick}
          onClose={() => setSearchOpen(false)}
        />
      )}
    </div>
  )
}

/** Minimal CSS.escape shim so attribute selectors work even on IDs with dots. */
function cssEscape(s: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') return CSS.escape(s)
  return s.replace(/[^a-zA-Z0-9_-]/g, (c) => `\\${c}`)
}
