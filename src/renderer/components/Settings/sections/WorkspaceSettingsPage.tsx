import React from 'react'
import { useCallback, useEffect, useState } from 'react'
import { Button, Input, Toggle, Slider } from '@/components/ui'
import { useSettingsStore, normalizeWorkspaceSwitchFocus } from '@/stores/settings'
import { SETTING_ROW_HINT, SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import { previewCompletionSound } from '@/lib/completion-sound'
import { COMPLETION_SOUNDS, normalizeCompletionSound } from '@/lib/completion-sound-options'
import { webFeatures } from '@/web/features'
import {
  parseSessionViewTab,
  parseSplitPaneView,
  type SessionViewTab,
  type SplitPaneView,
} from '@/components/SessionView/sessionViewTab'
import {
  THREAD_TEXT_WIDTH_DEFAULT,
  THREAD_TEXT_WIDTH_MAX_PX,
  THREAD_TEXT_WIDTH_MIN_PX,
  THREAD_TEXT_WIDTH_STEP_PX,
  isThreadTextWidthDefault,
} from '@/components/SessionView/threadTextWidth'
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

/** A slider's value, after the track. One width for every slider on the page,
 *  so the tracks end on the same line. */
const SLIDER_VALUE = 'w-12 text-right type-hint tabular-nums text-[var(--color-text-secondary)]'

/** Search group for every row on this page (Settings.tsx opens the page for it). */
export const WORKSPACE_SETTINGS_GROUP = 'Workspace settings'

export const WORKSPACE_SETTINGS_MANIFEST: SettingEntry[] = [
  { id: 'projects.workspace-settings', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Workspace settings', description: 'Settings for every workspace: the Active bar, the completion sound, auto-select, the view new agents open in, and how agents get their instructions', keywords: ['workspace', 'workspaces', 'settings', 'general', 'defaults', 'all workspaces', 'agents'] },
  { id: 'general.active-window-hours', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Active Bar window', description: 'How long workspaces stay Active after activity', keywords: ['active', 'bar', 'window', 'hours', 'tenure', 'workspace', 'recent', 'sidebar'] },
  { id: 'general.completion-sound', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound', description: 'Play a sound when an agent finishes unwatched', keywords: ['sound', 'chime', 'audio', 'notification', 'agent', 'done', 'finished', 'complete', 'unseen', 'orange', 'amber', 'dot'] },
  { id: 'general.completion-sound-choice', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound choice', description: 'Which sound plays when an agent finishes, with a Preview button', keywords: ['sound', 'chime', 'ping', 'ding', 'pop', 'tick', 'audio', 'preview', 'choose', 'notification'] },
  { id: 'general.completion-sound-volume', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Completion sound volume', description: 'How loud the sound is when an agent finishes', keywords: ['sound', 'volume', 'loud', 'quiet', 'level', 'ping', 'chime', 'audio'] },
  { id: 'general.workspace-switch-focus', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'When moving between workspaces, auto-select', description: 'Switching workspaces or project panes (⌘1–⌘9) focuses the terminal or the agent message box so you can type immediately', keywords: ['workspace', 'switch', 'focus', 'terminal', 'composer', 'message', 'agent', 'auto-select', 'input', 'pane', 'project'] },
  { id: 'general.new-agent-view', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'New agents open in', description: 'The view a new agent opens in: Terminal, Thread, Chatter, or Split view with the two sides you choose. Agents you already set a view for keep it.', keywords: ['new', 'agent', 'default', 'view', 'open', 'terminal', 'thread', 'chatter', 'split', 'split view', 'left', 'right', 'side', 'layout', 'workspace'] },
  { id: 'general.thread-text-width', section: 'projects', group: WORKSPACE_SETTINGS_GROUP, label: 'Thread text width', description: 'How wide the text gets in Thread view, centered in the pane, or Full width. On this computer only.', keywords: ['thread', 'text', 'width', 'column', 'readable', 'narrow', 'wide', 'full width', 'center', 'centre', 'limit', 'chat'] },
  // Graduated out of Experimental in 0.46.3 (Rosson 2026-10-21). Desktop
  // only: the hosted web client is single-server (MS56).
  ...(webFeatures.multiHost
    ? [{ id: 'general.remote-rooms-preview', section: 'projects' as const, group: WORKSPACE_SETTINGS_GROUP, label: "Open other servers' agents in Home", description: 'On: clicking an agent on Home that lives on another server opens it right here. Off: it switches this window to that server', keywords: ['home', 'remote', 'room', 'rooms', 'server', 'servers', 'other', 'multi', 'switch', 'open', 'agent', 'agents', 'here'] }]
    : []),
]

/** The Workspace settings page: the right-hand panel of Settings →
 *  Workspaces / Agents while "Workspace settings" is selected. */
export function WorkspaceSettingsPage(): React.JSX.Element {
  return (
    <div
      className="w-full space-y-6 pb-8"
      data-testid="workspace-settings-page"
      data-settings-id="projects.workspace-settings"
    >
      <div>
        <h2 className="type-title text-[var(--color-text-primary)]">Workspace settings</h2>
        <p className={SETTING_ROW_HINT}>
          How every workspace behaves: the Active bar in the sidebar, the
          sound when an agent finishes, where the cursor goes when you switch,
          and how new agents open.
        </p>
      </div>
      <SettingsGroup title="Every workspace" card>
        <WorkspaceBehaviorRows />
      </SettingsGroup>
      <AgentSkillsSection />
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
      <ThreadTextWidthRow />
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
    <SettingRow
      settingId="general.active-window-hours"
      label="Keep workspaces Active for"
      hint="How long a workspace stays in the sidebar's Active section after you last use it. At least 1 hour."
    >
      <Input
        type="number"
        min={1}
        step={1}
        value={draft}
        aria-label="Keep workspaces Active for, in hours"
        onChange={(e) => setDraft(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === 'Enter') {
            e.currentTarget.blur()
          }
        }}
        className="w-16 text-center type-control"
      />
      <span className="type-hint text-[var(--color-text-secondary)]">hours</span>
    </SettingRow>
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
    <>
      <SettingRow
        settingId="general.completion-sound"
        label="Play a sound when an agent finishes unwatched"
        hint="A soft chime when an agent finishes while you're looking elsewhere. Agents you're watching never chime."
      >
        <Toggle
          checked={enabled}
          onChange={(next) => void setCompletionSoundEnabled(next)}
          aria-label="Play a sound when an agent finishes unwatched"
        />
      </SettingRow>
      <SettingRow settingId="general.completion-sound-choice" label="Sound">
        <SettingDropdown
          value={sound}
          ariaLabel="Completion sound"
          options={COMPLETION_SOUNDS.map((s) => ({ value: s.id, label: s.label }))}
          onChange={(v) => void setCompletionSound(normalizeCompletionSound(v))}
        />
        <Button variant="quiet" onClick={() => previewCompletionSound(sound, volume)}>
          Preview
        </Button>
      </SettingRow>
      <SettingRow settingId="general.completion-sound-volume" label="Volume">
        <Slider
          min={0}
          max={100}
          step={1}
          value={volume}
          aria-label="Completion sound volume"
          onChange={(e) => setCompletionSoundVolume(parseInt(e.target.value, 10))}
          className="w-40"
        />
        <span className={SLIDER_VALUE} data-slider-value="">
          {volume}%
        </span>
      </SettingRow>
    </>
  )
}

// ── Workspace-switch focus target ──────────────────────────────────────
// When the user switches workspaces (icon rail / Active bar / workspace
// tab), focus the terminal or the compose bar so they can type immediately.
// Thin-client only (`k2.workspaceSwitchFocus` in localStorage) — not a
// daemon setting. `"composer"` (Message agent) is the default since 0.46.10
// (`WORKSPACE_SWITCH_FOCUS_DEFAULT`); a saved pick keeps winning. `"off"`
// moves nothing.
function WorkspaceSwitchFocusRow(): React.JSX.Element {
  const value = useSettingsStore((s) => s.workspaceSwitchFocus)
  const setWorkspaceSwitchFocus = useSettingsStore((s) => s.setWorkspaceSwitchFocus)

  return (
    <SettingRow
      settingId="general.workspace-switch-focus"
      label="When moving between workspaces, auto-select"
      hint="Where the cursor goes when you switch workspaces, open Home or press ⌘1–⌘9, so you can type right away."
    >
      <SettingDropdown
        value={value}
        options={[
          { value: 'terminal', label: 'Terminal' },
          { value: 'composer', label: 'Message agent' },
          { value: 'off', label: 'Nothing' },
        ]}
        onChange={(v) => setWorkspaceSwitchFocus(normalizeWorkspaceSwitchFocus(v))}
      />
    </SettingRow>
  )
}

// ── Thread text width ──────────────────────────────────────────────────
// How wide the text column may get in Thread view (Split and the Design tab
// keep theirs). Thin-client only (`k2.threadTextWidth`): a reading width is
// about one person's screen, not the server.
function ThreadTextWidthRow(): React.JSX.Element {
  const pref = useSettingsStore((s) => s.threadTextWidth)
  const setThreadTextWidth = useSettingsStore((s) => s.setThreadTextWidth)

  return (
    <div data-settings-id="general.thread-text-width" className="divide-y divide-[var(--color-border)]">
      <SettingRow
        label="Thread text width"
        hint="How wide messages get in Thread view, centered in the pane. On this computer only."
      >
        {isThreadTextWidthDefault(pref) ? null : (
          <Button
            variant="quiet"
            data-testid="thread-text-width-reset"
            onClick={() => setThreadTextWidth(THREAD_TEXT_WIDTH_DEFAULT)}
          >
            Reset
          </Button>
        )}
        <span className="type-hint text-[var(--color-text-secondary)]">Full width</span>
        <Toggle
          checked={!pref.limited}
          aria-label="Full width"
          onChange={(full) => setThreadTextWidth({ limited: !full, px: pref.px })}
        />
      </SettingRow>
      <SettingRow label="Width">
        <Slider
          min={THREAD_TEXT_WIDTH_MIN_PX}
          max={THREAD_TEXT_WIDTH_MAX_PX}
          step={THREAD_TEXT_WIDTH_STEP_PX}
          value={pref.px}
          disabled={!pref.limited}
          aria-label="Thread text width"
          onChange={(e) => setThreadTextWidth({ limited: pref.limited, px: parseInt(e.target.value, 10) })}
          className="w-40 disabled:opacity-50"
        />
        <span className={SLIDER_VALUE} data-slider-value="">
          {pref.px}px
        </span>
      </SettingRow>
    </div>
  )
}

// ── New agents open in ─────────────────────────────────────────────────
// The view a conversation opens in when this client has no remembered view
// for it (Terminal / Thread / Chatter / Split view — the same choices as the
// view button on an agent). Conversations with a remembered view keep it.
// Unset, it is Split view, Terminal left and Thread right, since 0.46.10
// (`NEW_AGENT_VIEW_DEFAULT`).
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
    <div data-settings-id="general.new-agent-view" className="divide-y divide-[var(--color-border)]">
      <SettingRow
        label="New agents open in"
        hint="The view an agent opens in the first time you see it here. Agents you have already set a view for keep it. Projects use their own setting (Project settings → Agents open in)."
      >
        <SettingDropdown
          value={pref.view}
          ariaLabel="New agents open in"
          options={NEW_AGENT_VIEW_OPTIONS}
          onChange={(v) =>
            setNewAgentView({ view: parseSessionViewTab(v), split: pref.split })
          }
        />
      </SettingRow>
      {isSplit ? (
        <div data-testid="new-agent-view-split" className="divide-y divide-[var(--color-border)]">
          <SettingRow label="Left side">
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
          </SettingRow>
          <SettingRow label="Right side">
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
          </SettingRow>
        </div>
      ) : null}
    </div>
  )
}
