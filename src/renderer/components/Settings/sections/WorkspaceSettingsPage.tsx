import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { Toggle } from '@/components/ui'
import { useSettingsStore, normalizeWorkspaceSwitchFocus } from '@/stores/settings'
import { SettingDropdown } from '../controls/SettingControls'
import { previewCompletionSound } from '@/lib/completion-sound'
import { COMPLETION_SOUNDS, normalizeCompletionSound } from '@/lib/completion-sound-options'
import { webFeatures } from '@/web/features'
import {
  parseSessionViewTab,
  parseSplitPaneView,
  type SessionViewTab,
  type SplitPaneView,
} from '@/components/SessionView/sessionViewTab'
import type { SettingEntry } from '../searchManifest'
import { AgentSkillsSection } from './AgentSkillsSection'
import { RemoteRoomsPreviewRow } from './RemoteRoomsPreviewRow'

// ── Workspace settings (Settings → Workspaces / Agents → Workspace settings)
// Moved out of Settings → General → Workspaces in 0.46.5 (Rosson
// 2026-10-27: "a page that you can visit on the Workspace / agents page by
// putting it above the focus groups toggle"). The rows, their stores and
// their daemon reads/writes are the same ones General used; only the page
// that holds them moved. The search ids stay `general.*` so saved links and
// tests that name a row keep working; their section is now `projects` and
// their group `Workspace settings`, which is how search lands here.

/** Search group for every row on this page (Settings.tsx opens the page for it). */
export const WORKSPACE_SETTINGS_GROUP = 'Workspace settings'

export const WORKSPACE_SETTINGS_MANIFEST: SettingEntry[] = [
  { id: 'projects.workspace-settings', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Workspace settings', description: 'Settings for every workspace on this client: the Active bar, the completion sound, auto-select, the view new agents open in, and the Canonical Agent Flow guide', keywords: ['workspace', 'workspaces', 'settings', 'general', 'defaults', 'all workspaces', 'agents'] },
  { id: 'general.active-window-hours', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Active Bar window', description: 'How long workspaces stay Active after activity', keywords: ['active', 'bar', 'window', 'hours', 'tenure', 'workspace', 'recent', 'sidebar'] },
  { id: 'general.completion-sound', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound', description: 'Play a sound when an agent finishes unwatched', keywords: ['sound', 'chime', 'audio', 'notification', 'agent', 'done', 'finished', 'complete', 'unseen', 'orange', 'amber', 'dot'] },
  { id: 'general.completion-sound-choice', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound choice', description: 'Which sound plays when an agent finishes, with a Preview button', keywords: ['sound', 'chime', 'ping', 'ding', 'pop', 'tick', 'audio', 'preview', 'choose', 'notification'] },
  { id: 'general.completion-sound-volume', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound volume', description: 'How loud the sound is when an agent finishes', keywords: ['sound', 'volume', 'loud', 'quiet', 'level', 'ping', 'chime', 'audio'] },
  { id: 'general.workspace-switch-focus', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'When moving between workspaces, auto-select', description: 'Switching workspaces or project panes (⌘1–⌘9) focuses the terminal or the agent message box so you can type immediately', keywords: ['workspace', 'switch', 'focus', 'terminal', 'composer', 'message', 'agent', 'auto-select', 'input', 'pane', 'project'] },
  { id: 'general.new-agent-view', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'New agents open in', description: 'The view a new agent opens in: Terminal, Thread, Chatter, or Split view with the two sides you choose. Agents you already set a view for keep it.', keywords: ['new', 'agent', 'default', 'view', 'open', 'terminal', 'thread', 'chatter', 'split', 'split view', 'left', 'right', 'side', 'layout', 'workspace'] },
  // Graduated out of Experimental in 0.46.3 (Rosson 2026-10-21). Desktop
  // only: the hosted web client is single-server (MS56).
  ...(webFeatures.multiHost
    ? [{ id: 'general.remote-rooms-preview', section: 'projects' as const, group: WORKSPACE_SETTINGS_GROUP, label: "Open other servers' agents in Home (don't switch servers)", description: 'On: clicking an agent on Home that lives on another server opens it right here. Off: it switches this window to that server', keywords: ['home', 'remote', 'room', 'rooms', 'server', 'servers', 'other', 'multi', 'switch', 'open', 'agent', 'agents', 'here'] }]
    : []),
]

/** The Workspace settings page: the right-hand panel of Settings →
 *  Workspaces / Agents while "Workspace settings" is selected. */
export function WorkspaceSettingsPage(): React.JSX.Element {
  return (
    <div
      className="flex flex-col h-full min-h-0 w-full"
      data-testid="workspace-settings-page"
      data-settings-id="projects.workspace-settings"
    >
      <div className="flex-shrink-0 pb-3 pr-1 w-full border-b border-[var(--color-border)]">
        <h2 className="text-base font-medium text-[var(--color-text-primary)]">Workspace settings</h2>
        <p className="text-[11px] text-[var(--color-text-muted)] mt-1">
          How every workspace behaves on this client: the sidebar Active bar,
          the sound when an agent finishes out of view, what gets focus when
          you switch, and the view new agents open in.
        </p>
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto pt-4 pb-8 pr-1 [scrollbar-gutter:stable]">
        <div className="space-y-6 w-full">
          <div className="space-y-4 max-w-xl">
            <WorkspaceBehaviorRows />
          </div>
          {/* Full column for the Canonical Agent Flow diagram + guide */}
          <div className="border-t border-[var(--color-border)] pt-5 w-full">
            <AgentSkillsSection />
          </div>
        </div>
      </div>
    </div>
  )
}

/** Workspace settings rows, in order. Exported for tests. */
export function WorkspaceBehaviorRows(): React.JSX.Element {
  return (
    <>
      <ActiveWindowHoursRow />
      <CompletionSoundRow />
      <WorkspaceSwitchFocusRow />
      <NewAgentViewRow />
      {/* Graduated from Experimental in 0.46.3. Desktop only (MS56). */}
      {webFeatures.multiHost && <RemoteRoomsPreviewRow />}
    </>
  )
}

// ── Active Bar window (P1.C) ───────────────────────────────────────────
// How long a workspace stays in the sidebar's Active section after the
// user last interacted with it (Active-Bar rule 2). Default 24h, min 1h.
// Backed by `settings.activeWindowHours` (persisted via the daemon's
// app_settings deep-merge); read by ActiveBar and a future reaper.
function ActiveWindowHoursRow(): React.JSX.Element {
  const activeWindowHours = useSettingsStore((s) => s.activeWindowHours)
  const setActiveWindowHours = useSettingsStore((s) => s.setActiveWindowHours)
  // Local draft so the user can clear/type freely; commit (clamped) on blur
  // or Enter. Mirrors the typed-input ergonomics of the terminal settings.
  const [draft, setDraft] = useState<string>(String(activeWindowHours))

  useEffect(() => {
    setDraft(String(activeWindowHours))
  }, [activeWindowHours])

  const commit = useCallback(() => {
    const parsed = parseInt(draft, 10)
    const next = Number.isFinite(parsed) ? Math.max(1, parsed) : activeWindowHours
    setDraft(String(next))
    if (next !== activeWindowHours) {
      void setActiveWindowHours(next)
    }
  }, [draft, activeWindowHours, setActiveWindowHours])

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.active-window-hours"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Keep workspaces Active for
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          How long a workspace stays in the sidebar&apos;s Active section after
          you last interacted with it. Minimum 1 hour.
        </p>
      </div>
      <div className="flex items-center gap-2 flex-shrink-0">
        <input
          type="number"
          min={1}
          step={1}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.currentTarget.blur()
            }
          }}
          // colorScheme:dark renders the native up/down stepper carrots light
          // (visible) instead of the default dark-on-dark black — matches the
          // K2 Connect section's number stepper.
          style={{ colorScheme: 'dark' }}
          className="w-16 px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag text-center"
        />
        <span className="text-[10px] text-[var(--color-text-muted)]">hours</span>
      </div>
    </div>
  )
}

// ── Completion sound (F4) ──────────────────────────────────────────────
// Chimes ONLY for UNSEEN completions — an agent finished while its pane
// wasn't being watched (the same fire that lights the Active-bar amber
// dot). Watched panes never chime. Backed by
// `settings.completionSoundEnabled` (daemon settings.json deep-merge),
// default ON. See .k2/notes/orange-dot-done-sound.md.
//
// 0.46.1: the sound and its volume (`completionSound`,
// `completionSoundVolume` 0–100, 50 = the pre-0.46.1 level) sit under the
// switch. Global daemon settings, so every client plays the same chime at
// the same level; per-workspace bells stay on/off. Preview plays the
// chosen sound at the chosen volume, even with the switch off.
function CompletionSoundRow(): React.JSX.Element {
  const enabled = useSettingsStore((s) => s.completionSoundEnabled)
  const setCompletionSoundEnabled = useSettingsStore((s) => s.setCompletionSoundEnabled)
  const sound = useSettingsStore((s) => s.completionSound)
  const setCompletionSound = useSettingsStore((s) => s.setCompletionSound)
  const volume = useSettingsStore((s) => s.completionSoundVolume)
  const setCompletionSoundVolume = useSettingsStore((s) => s.setCompletionSoundVolume)

  return (
    <div className="border-b border-[var(--color-border)]">
      <div
        className="flex items-center justify-between py-2"
        data-settings-id="general.completion-sound"
      >
        <div className="flex-1 min-w-0 mr-3">
          <span className="text-xs text-[var(--color-text-secondary)]">
            Play a sound when an agent finishes unwatched
          </span>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            A soft chime when an agent completes while you&apos;re looking
            elsewhere — the audible twin of the amber dot in the Active bar.
            Agents you&apos;re watching never chime.
          </p>
        </div>
        <Toggle
          checked={enabled}
          onChange={(next) => void setCompletionSoundEnabled(next)}
          aria-label="Play a sound when an agent finishes unwatched"
        />
      </div>
      <div className="pl-3 pb-2 space-y-1">
        <div
          className="flex items-center justify-between py-1"
          data-settings-id="general.completion-sound-choice"
        >
          <span className="text-[11px] text-[var(--color-text-muted)]">Sound</span>
          <div className="flex items-center gap-2">
            <SettingDropdown
              value={sound}
              ariaLabel="Completion sound"
              options={COMPLETION_SOUNDS.map((s) => ({ value: s.id, label: s.label }))}
              onChange={(v) => void setCompletionSound(normalizeCompletionSound(v))}
            />
            <button
              type="button"
              onClick={() => previewCompletionSound(sound, volume)}
              className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
            >
              Preview
            </button>
          </div>
        </div>
        <div
          className="flex items-center justify-between py-1"
          data-settings-id="general.completion-sound-volume"
        >
          <span className="text-[11px] text-[var(--color-text-muted)]">Volume</span>
          <div className="flex items-center gap-3">
            <input
              type="range"
              min={0}
              max={100}
              step={1}
              value={volume}
              aria-label="Completion sound volume"
              onChange={(e) => setCompletionSoundVolume(parseInt(e.target.value, 10))}
              className="w-40 no-drag k2so-slider"
            />
            <span className="w-9 text-right text-[11px] tabular-nums text-[var(--color-text-secondary)]">
              {volume}%
            </span>
          </div>
        </div>
      </div>
    </div>
  )
}

// ── Workspace-switch focus target ──────────────────────────────────────
// When the user switches workspaces (icon rail / Active bar / workspace
// tab), focus the terminal or the compose bar so they can type immediately.
// Thin-client only (`k2.workspaceSwitchFocus` in localStorage) — not a
// daemon setting. `"terminal"` is the default; `"off"` moves nothing.
function WorkspaceSwitchFocusRow(): React.JSX.Element {
  const value = useSettingsStore((s) => s.workspaceSwitchFocus)
  const setWorkspaceSwitchFocus = useSettingsStore((s) => s.setWorkspaceSwitchFocus)

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.workspace-switch-focus"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          When moving between workspaces, auto-select
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          Switching workspaces, opening Home, and ⌘1–⌘9 focus the terminal
          or the agent message box (the Thread's when it is open) so you can
          type immediately.
        </p>
      </div>
      <SettingDropdown
        value={value}
        options={[
          { value: 'terminal', label: 'Terminal' },
          { value: 'composer', label: 'Message agent' },
          { value: 'off', label: 'Nothing' },
        ]}
        onChange={(v) => setWorkspaceSwitchFocus(normalizeWorkspaceSwitchFocus(v))}
      />
    </div>
  )
}

// ── New agents open in ─────────────────────────────────────────────────
// The view a conversation opens in when this client has no remembered view
// for it (Terminal / Thread / Chatter / Split view — the same choices as the
// view button on an agent). Conversations with a remembered view keep it.
// Thin-client only (`k2.newAgentView` in localStorage), like the auto-select
// row above and the per-conversation view memory. No Chat choice: the Chat
// view is gone (deleted 0.46.5); a saved Chat pick reads back as Thread and
// is written back (`readNewAgentView`).
const NEW_AGENT_VIEW_OPTIONS: { value: SessionViewTab; label: string }[] = [
  { value: 'terminal', label: 'Terminal' },
  { value: 'thread', label: 'Thread' },
  { value: 'chatter', label: 'Chatter' },
  { value: 'split', label: 'Split view' },
]

const SPLIT_SIDE_OPTIONS: { value: SplitPaneView; label: string }[] = [
  { value: 'terminal', label: 'Terminal' },
  { value: 'thread', label: 'Thread' },
  { value: 'chatter', label: 'Chatter' },
]

function NewAgentViewRow(): React.JSX.Element {
  const pref = useSettingsStore((s) => s.newAgentView)
  const setNewAgentView = useSettingsStore((s) => s.setNewAgentView)
  const isSplit = pref.view === 'split'

  return (
    <div data-settings-id="general.new-agent-view">
      <div className="flex items-center justify-between py-2">
        <div className="flex-1 min-w-0 mr-3">
          <span className="text-xs text-[var(--color-text-secondary)]">
            New agents open in
          </span>
          <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
            The view an agent opens in the first time you see it here. Agents
            you have already set a view for keep it.
          </p>
        </div>
        <SettingDropdown
          value={pref.view}
          ariaLabel="New agents open in"
          options={NEW_AGENT_VIEW_OPTIONS}
          onChange={(v) =>
            setNewAgentView({ view: parseSessionViewTab(v), split: pref.split })
          }
        />
      </div>
      {isSplit ? (
        <div className="pl-3 pb-2 space-y-1" data-testid="new-agent-view-split">
          <div className="flex items-center justify-between py-1">
            <span className="text-[11px] text-[var(--color-text-muted)]">Left side</span>
            <SettingDropdown
              value={pref.split.left}
              ariaLabel="Split view left side"
              options={SPLIT_SIDE_OPTIONS.map((o) => ({
                ...o,
                disabled: o.value === pref.split.right,
              }))}
              onChange={(v) => {
                const left = parseSplitPaneView(v, pref.split.left)
                if (left === pref.split.right) return
                setNewAgentView({ view: 'split', split: { left, right: pref.split.right } })
              }}
            />
          </div>
          <div className="flex items-center justify-between py-1">
            <span className="text-[11px] text-[var(--color-text-muted)]">Right side</span>
            <SettingDropdown
              value={pref.split.right}
              ariaLabel="Split view right side"
              options={SPLIT_SIDE_OPTIONS.map((o) => ({
                ...o,
                disabled: o.value === pref.split.left,
              }))}
              onChange={(v) => {
                const right = parseSplitPaneView(v, pref.split.right)
                if (right === pref.split.left) return
                setNewAgentView({ view: 'split', split: { left: pref.split.left, right } })
              }}
            />
          </div>
        </div>
      ) : null}
    </div>
  )
}
