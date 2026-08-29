import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { SettingsSection } from '@/stores/settings'
import { SectionErrorBoundary } from './SectionErrorBoundary'
import { SettingsSearchModal } from './SettingsSearchModal'
import type { SettingEntry } from './searchManifest'
import { GeneralSection, GeneralRemoteHostPanel, GENERAL_MANIFEST } from './sections/GeneralSection'
import { StylesSection, STYLES_MANIFEST } from './sections/StylesSection'
import { useConnectHostStore } from '@/stores/connect-host'
import DesktopChromeLeft from '@/components/TopBar/DesktopChromeLeft'
import DesktopChromeRight from '@/components/TopBar/DesktopChromeRight'
import { titleBarDragOnMouseDown, titleBarOnDoubleClick } from '@/lib/titlebar-drag'
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
import { ApiTokensSection, API_TOKENS_MANIFEST } from './sections/ApiTokensSection'
import { ProjectsSection, PROJECTS_MANIFEST } from './sections/ProjectsSection'
import { EmailHostingSection, EMAIL_HOSTING_MANIFEST } from './sections/EmailHostingSection'
import { EmailLinkSection, EMAIL_LINK_MANIFEST } from './sections/EmailLinkSection'
import { DataSection, DATA_MANIFEST } from './sections/DataSection'
// The Projects (project GROUPS) section — §6.5 relocation. NOT to be
// confused with ProjectsSection above, the LEGACY workspaces section
// (id 'projects', label "Workspaces").
import ProjectGroupSettings from '../Projects/ProjectSettings'
import { ContextCatalogSection, CONTEXT_CATALOG_MANIFEST } from './sections/ContextCatalogSection'
import { AGENT_SKILLS_MANIFEST } from './sections/AgentSkillsSection'
// HeartbeatsPanel is rendered inline inside ProjectsSection now; manifest
// stays exported from HeartbeatsSection so searches still find it.
import { HEARTBEATS_MANIFEST } from './sections/HeartbeatsSection'
import { WakeSchedulerSection, WAKE_SCHEDULER_MANIFEST } from './sections/WakeSchedulerSection'
import { PermissionsSection, PERMISSIONS_MANIFEST } from './sections/PermissionsSection'
import { DictationLabSection, DICTATION_LAB_MANIFEST } from './sections/DictationLabSection'
import ServerSwitcher from '../TopBar/ServerSwitcher'
import PageTabs from '../TopBar/PageTabs'
import TimerButton from '@/components/Timer/TimerButton'
import K2NounsCheatSheet from '@/components/CheatSheet/K2NounsCheatSheet'
import ModeToggle from '@/components/Presence/ModeToggle'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { webFeatures } from '@/web/features'
import { isAirgap } from '@/lib/airgap'

// ── Section nav ──────────────────────────────────────────────────────
// Top-level items, then titled groups with indented children.
type NavLeaf = {
  id?: SettingsSection
  label: string
  soon?: boolean
  hide?: boolean
}
type NavBlock =
  | { kind: 'item'; id: SettingsSection; label: string }
  | { kind: 'group'; title: string; items: NavLeaf[] }

function settingsNav(): NavBlock[] {
  const hideTunnel = isAirgap()
  const blocks: NavBlock[] = [
    { kind: 'item', id: 'general', label: 'General' },
    { kind: 'item', id: 'styles', label: 'Styles' },
    { kind: 'item', id: 'agents', label: 'LLMs' },
    { kind: 'item', id: 'projects', label: 'Workspaces / Agents' },
    { kind: 'item', id: 'project-groups', label: 'Projects' },
    { kind: 'item', id: 'context-catalog', label: 'Context Catalog' },
    { kind: 'item', id: 'email-link', label: 'Email Link' },
    { kind: 'item', id: 'keybindings', label: 'Key Bindings' },
    ...(webFeatures.permissions
      ? ([{ kind: 'item', id: 'permissions' as const, label: 'Accessibility' }] satisfies NavBlock[])
      : []),
    {
      kind: 'group',
      title: 'K2 Server',
      items: [
        { id: 'k2-connect', label: 'Tunnel', hide: hideTunnel },
        { id: 'k2-access', label: 'Server Access' },
        { id: 'connections', label: 'Connected Servers' },
        { id: 'api-tokens', label: 'API Keys' },
        { id: 'companion', label: 'K2 Companion' },
      ],
    },
    {
      kind: 'group',
      title: 'Editors / Fonts',
      items: [
        { id: 'terminal', label: 'Terminal' },
        { id: 'code-editor', label: 'Code' },
        { id: 'editors', label: 'Defaults' },
      ],
    },
    {
      kind: 'group',
      title: 'Logs',
      items: [
        { id: 'wake-scheduler', label: 'Heartbeats' },
        { id: 'timer', label: 'Timer' },
      ],
    },
    {
      kind: 'group',
      title: 'Sidecars',
      items: [
        { id: 'email-hosting', label: 'Email Hosting' },
        { id: 'data', label: 'Database' },
        { label: 'Skin Access', soon: true },
      ],
    },
  ]
  if (import.meta.env.DEV) {
    blocks.push({ kind: 'item', id: 'dictation-lab', label: 'Dictation Lab (dev)' })
  }
  return blocks
}

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
      ...CONTEXT_CATALOG_MANIFEST,
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
      ...API_TOKENS_MANIFEST,
      ...EMAIL_HOSTING_MANIFEST,
      ...EMAIL_LINK_MANIFEST,
      ...DATA_MANIFEST,
      ...WAKE_SCHEDULER_MANIFEST,
      ...(webFeatures.permissions ? PERMISSIONS_MANIFEST : []),
      ...DICTATION_LAB_MANIFEST,
    ],
    [],
  )

  // Hosted web: if a prior session left activeSection on amputated
  // Permissions, bounce to General so the content pane isn't blank.
  // Air-gap: Tunnel (k2-connect) hits Supabase — land on Connected Servers.
  useEffect(() => {
    if (!webFeatures.permissions && activeSection === 'permissions') {
      setSection('general')
    }
    if (isAirgap() && activeSection === 'k2-connect') {
      setSection('connections')
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
      {/* Top-bar — same left cluster as the other pages so Agents /
          Projects / Tickets stay reachable (settings cog is the
          selected tab). */}
      <div
        className="flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none flex-shrink-0"
        onMouseDown={titleBarDragOnMouseDown}
        onDoubleClick={titleBarOnDoubleClick}
        style={{ height: TOPBAR_HEIGHT, minHeight: TOPBAR_HEIGHT }}
      >
        <div className="flex items-center gap-2">
          <DesktopChromeLeft />
          {/* App name (in-app wordmark) */}
          <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">
            K2
          </span>
          {/* K2 server switcher (Local / saved servers / add) */}
          <ServerSwitcher />
          <PageTabs />
        </div>
        <DesktopChromeRight>
          <div className="flex items-center gap-1 no-drag">
            <TimerButton />
            <K2NounsCheatSheet />
            <ModeToggle />
          </div>
        </DesktopChromeRight>
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
          {settingsNav().map((block) => {
            if (block.kind === 'item') {
              return (
                <SettingsNavButton
                  key={block.id}
                  id={block.id}
                  label={block.label}
                  active={activeSection === block.id}
                  onClick={() => setSection(block.id)}
                />
              )
            }
            const items = block.items.filter((it) => !it.hide)
            if (items.length === 0) return null
            return (
              <div key={block.title} className="mt-2 first:mt-0">
                <div className="px-4 pt-2 pb-1 text-[10px] font-medium uppercase tracking-wider text-[var(--color-text-muted)]">
                  {block.title}
                </div>
                {items.map((it) => {
                  if (it.soon || !it.id) {
                    return (
                      <div
                        key={it.label}
                        className="w-full text-left pl-7 pr-4 py-1.5 text-xs text-[var(--color-text-muted)] flex items-center gap-2"
                      >
                        <span>{it.label}</span>
                        <span className="text-[8px] uppercase tracking-wider font-semibold px-1 py-0.5 border border-[var(--color-border)]">
                          soon
                        </span>
                      </div>
                    )
                  }
                  const id = it.id
                  return (
                    <SettingsNavButton
                      key={id}
                      id={id}
                      label={it.label}
                      nested
                      active={activeSection === id}
                      onClick={() => setSection(id)}
                    />
                  )
                })}
              </div>
            )
          })}
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
          activeSection === 'context-catalog' ||
          activeSection === 'k2-connect' ||
          activeSection === 'k2-access' ||
          activeSection === 'connections' ||
          activeSection === 'agents' ||
          activeSection === 'email-hosting' ||
          activeSection === 'email-link' ||
          activeSection === 'data'
            ? 'overflow-hidden p-0'
            : activeSection === 'dictation-lab'
              ? 'overflow-hidden p-6'
              : 'overflow-y-auto p-6'
        }`}
      >
        {activeSection === 'general' && (
          /* General owns its own chrome (full-width tabs) + scroll.
             Remote: half/half with Connected host controls on the right. */
          <div className="flex h-full min-h-0">
            <div className="flex-1 min-w-0 min-h-0 overflow-hidden flex flex-col">
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
        {(activeSection === 'k2-connect' ||
          activeSection === 'k2-access' ||
          activeSection === 'connections') && (
          <SectionErrorBoundary>
            {/* Host | Servers primary tabs (full width). Deep-link
                `connections` opens Servers; `k2-connect` opens Host. */}
            <K2ConnectSettingsShell />
          </SectionErrorBoundary>
        )}
        {activeSection === 'api-tokens' && (
          <SectionErrorBoundary>
            <ApiTokensSection />
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
        {activeSection === 'context-catalog' && (
          <SectionErrorBoundary>
            <ContextCatalogSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'email-hosting' && (
          <SectionErrorBoundary>
            <EmailHostingSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'email-link' && (
          <SectionErrorBoundary>
            {/* h-full so Gmail OAuth BrowserPane dock gets a real height. */}
            <div className="h-full min-h-0 flex flex-col">
              <EmailLinkSection />
            </div>
          </SectionErrorBoundary>
        )}
        {activeSection === 'data' && (
          <SectionErrorBoundary>
            <DataSection />
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

function SettingsNavButton({
  id,
  label,
  active,
  nested,
  onClick,
}: {
  id: SettingsSection
  label: string
  active: boolean
  nested?: boolean
  onClick: () => void
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-settings-nav={id}
      onClick={onClick}
      className={`w-full text-left ${nested ? 'pl-7 pr-4' : 'px-4'} py-1.5 text-xs no-drag cursor-pointer transition-colors ${
        active
          ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
      }`}
    >
      {label}
    </button>
  )
}

/** Minimal CSS.escape shim so attribute selectors work even on IDs with dots. */
function cssEscape(s: string): string {
  if (typeof CSS !== 'undefined' && typeof CSS.escape === 'function') return CSS.escape(s)
  return s.replace(/[^a-zA-Z0-9_-]/g, (c) => `\\${c}`)
}
