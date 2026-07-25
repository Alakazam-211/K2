import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { SettingsSection } from '@/stores/settings'
import { SectionErrorBoundary } from './SectionErrorBoundary'
import { SettingsSearchModal } from './SettingsSearchModal'
import type { SettingEntry } from './searchManifest'
import { GeneralSection, GeneralRemoteHostPanel, GENERAL_MANIFEST } from './sections/GeneralSection'
import { StylesSection, STYLES_MANIFEST } from './sections/StylesSection'
import { useConnectHostStore } from '@/stores/connect-host'
import TrafficLightSpacer from '@/components/TopBar/TrafficLightSpacer'
import { TerminalSection, TERMINAL_MANIFEST } from './sections/TerminalSection'
import { CodeEditorSettingsSection, CODE_EDITOR_MANIFEST } from './sections/CodeEditorSettingsSection'
import { EditorsSection, EDITORS_MANIFEST } from './sections/EditorsSection'
import { AgentsSection, AGENTS_MANIFEST } from './sections/AgentsSection'
import { KeybindingsSection, KEYBINDINGS_MANIFEST } from './sections/KeybindingsSection'
import { TimerSection, TIMER_MANIFEST } from './sections/TimerSection'
import { CompanionSection, COMPANION_MANIFEST } from './sections/CompanionSection'
import { CONNECTIONS_MANIFEST } from './sections/ConnectionsSection'
import { K2_CONNECT_MANIFEST } from './sections/K2ConnectSection'
import { K2ConnectSettingsShell } from './sections/K2ConnectSettingsShell'
import { ProjectsSection, PROJECTS_MANIFEST } from './sections/ProjectsSection'
import { EmailHostingSection, EMAIL_HOSTING_MANIFEST } from './sections/EmailHostingSection'
import { EmailLinkSection, EMAIL_LINK_MANIFEST } from './sections/EmailLinkSection'
// The Projects (project GROUPS) section — §6.5 relocation. NOT to be
// confused with ProjectsSection above, the LEGACY workspaces section
// (id 'projects', label "Workspaces").
import ProjectGroupSettings from '../Projects/ProjectSettings'
import { AGENT_SKILLS_MANIFEST } from './sections/AgentSkillsSection'
// HeartbeatsPanel is rendered inline inside ProjectsSection now; manifest
// stays exported from HeartbeatsSection so searches still find it.
import { HEARTBEATS_MANIFEST } from './sections/HeartbeatsSection'
import { WakeSchedulerSection, WAKE_SCHEDULER_MANIFEST } from './sections/WakeSchedulerSection'
import { PermissionsSection, PERMISSIONS_MANIFEST } from './sections/PermissionsSection'
import { DictationLabSection, DICTATION_LAB_MANIFEST } from './sections/DictationLabSection'
import ServerSwitcher from '../TopBar/ServerSwitcher'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { webFeatures } from '@/web/features'

// ── Section nav items ────────────────────────────────────────────────
// Agentic systems are always on. Canonical Agent Flow lives under
// General → Workspaces (not a top-level nav item).
const SECTIONS: { id: SettingsSection; label: string }[] = [
  { id: 'general', label: 'General' },
  { id: 'styles', label: 'Styles' },
  { id: 'agents', label: 'LLMs' },
  { id: 'projects', label: 'Workspaces / Agents' },
  { id: 'project-groups', label: 'Projects' },
  { id: 'k2-connect', label: 'K2 Connect' },
  { id: 'companion', label: 'K2 Companion' },
  { id: 'email-hosting', label: 'Email Hosting' },
  { id: 'email-link', label: 'Email Link' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'code-editor', label: 'Code Editor' },
  { id: 'editors', label: 'Editors' },
  { id: 'wake-scheduler', label: 'Heartbeats' },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'timer', label: 'Timer' },
  // Hosted web: macOS FDA / Accessibility etc. do not apply — omit nav entry.
  ...(webFeatures.permissions
    ? ([{ id: 'permissions' as const, label: 'Permissions' }] as const)
    : []),
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

  // Flat manifest across every section (agentic sections always included).
  const allEntries = useMemo<SettingEntry[]>(
    () => [
      ...GENERAL_MANIFEST,
      ...STYLES_MANIFEST,
      ...PROJECTS_MANIFEST,
      ...AGENT_SKILLS_MANIFEST,
      ...HEARTBEATS_MANIFEST,
      ...TERMINAL_MANIFEST,
      ...CODE_EDITOR_MANIFEST,
      ...EDITORS_MANIFEST,
      ...AGENTS_MANIFEST,
      ...KEYBINDINGS_MANIFEST,
      ...TIMER_MANIFEST,
      ...COMPANION_MANIFEST,
      ...CONNECTIONS_MANIFEST,
      ...K2_CONNECT_MANIFEST,
      ...EMAIL_HOSTING_MANIFEST,
      ...EMAIL_LINK_MANIFEST,
      ...WAKE_SCHEDULER_MANIFEST,
      ...(webFeatures.permissions ? PERMISSIONS_MANIFEST : []),
      ...DICTATION_LAB_MANIFEST,
    ],
    [],
  )

  // Hosted web: if a prior session left activeSection on amputated
  // Permissions, bounce to General so the content pane isn't blank.
  useEffect(() => {
    if (!webFeatures.permissions && activeSection === 'permissions') {
      setSection('general')
    }
  }, [activeSection, setSection])

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
    // General sub-tabs: search entries carry a group so we open the right tab
    // (e.g. Canonical Agent Flow → Workspaces) before scrolling to the row.
    if (entry.section === 'general' && entry.group) {
      const subByGroup: Record<string, 'general' | 'workspaces' | 'server' | 'local-llm'> = {
        General: 'general',
        Workspaces: 'workspaces',
        Server: 'server',
        'Local LLM': 'local-llm',
      }
      const sub = subByGroup[entry.group]
      if (sub) useSettingsStore.setState({ generalSubTab: sub })
    }
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
          <TrafficLightSpacer />
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
          {SECTIONS.map((s) => (
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
          activeSection === 'styles' ||
          activeSection === 'projects' ||
          activeSection === 'project-groups' ||
          activeSection === 'k2-connect' ||
          activeSection === 'connections' ||
          activeSection === 'agents' ||
          activeSection === 'email-hosting' ||
          activeSection === 'email-link'
            ? 'overflow-hidden p-0'
            : activeSection === 'dictation-lab'
              ? 'overflow-hidden p-6'
              : 'overflow-y-auto p-6'
        }`}
      >
        {activeSection === 'general' && (
          /* When local: full width (Canonical Agent Flow needs room). When
             remote: half/half with Connected host controls on the right. */
          <div className="flex h-full min-h-0">
            <div className="flex-1 min-w-0 overflow-y-auto p-6">
              <GeneralSection />
            </div>
            {isRemote && (
              <div className="flex-1 min-w-0 overflow-y-auto p-6 border-l border-[var(--color-border)]">
                <SectionErrorBoundary>
                  <GeneralRemoteHostPanel />
                </SectionErrorBoundary>
              </div>
            )}
          </div>
        )}
        {activeSection === 'styles' && (
          <SectionErrorBoundary>
            <StylesSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'terminal' && <TerminalSection />}
        {activeSection === 'code-editor' && <CodeEditorSettingsSection />}
        {activeSection === 'editors' && (
          <SectionErrorBoundary>
            <EditorsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'agents' && (
          <SectionErrorBoundary>
            <AgentsSection />
          </SectionErrorBoundary>
        )}
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
          <SectionErrorBoundary>
            {/* Host | Servers primary tabs (full width). Deep-link
                `connections` opens Servers; `k2-connect` opens Host. */}
            <K2ConnectSettingsShell />
          </SectionErrorBoundary>
        )}
        {activeSection === 'projects' && (
          <SectionErrorBoundary>
            <ProjectsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'project-groups' && (
          <SectionErrorBoundary>
            <ProjectGroupSettings />
          </SectionErrorBoundary>
        )}
        {activeSection === 'email-hosting' && (
          <SectionErrorBoundary>
            <EmailHostingSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'email-link' && (
          <SectionErrorBoundary>
            <EmailLinkSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'wake-scheduler' && (
          <SectionErrorBoundary>
            <WakeSchedulerSection />
          </SectionErrorBoundary>
        )}
        {webFeatures.permissions && activeSection === 'permissions' && (
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
