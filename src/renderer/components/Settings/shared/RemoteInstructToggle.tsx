import React, { useState } from 'react'
import { daemonCliPostQuery } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import type { ProjectWithWorkspaces } from '@/stores/projects'

/** Per-agent switch label (Rosson 2026-10-16). */
export const AGENT_SWITCH_LABEL = 'Let agents on other servers message this agent'
/** Shown while the per-agent switch is off. */
export const AGENT_SWITCH_OFF_TEXT =
  "Off: this agent isn't listed to connected servers and their agents can't message it. People who log in here can always message it."
/** Shown while the per-agent switch is on. */
export const AGENT_SWITCH_ON_TEXT =
  'On: this agent is listed to connected servers and their agents can message it. People who log in here can always message it.'
/** Shown under an OFF per-agent switch while the app master is on. */
export const AGENT_SWITCH_OVERRIDE_TEXT =
  'On anyway: "Let agents on other servers message every agent" (Settings → K2 Connect) is on, so this agent is listed to connected servers and their agents can message it.'

// ── #67 per-agent "Let agents on other servers message this agent" ────
// Internal name: remote-instruct (`projects.allow_remote_instruct`, written
// via `/cli/remote-instruct`). Since 2026-10-16 it covers FEDERATED agents
// only: off → the agent is not in the roster a paired server sees and
// federated live messages to it are declined. People who log in here (the
// owner, Admins and Members) can always message the agent — their role
// decides it on the daemon, not this switch. The app master
// (`allowRemoteInstruct`, Settings → K2 Connect) OR-overrides it.
// Renders in Settings → Workspaces → Agent (bordered box with the full
// description) and in the room's Agent drawer tab (`compact`: the label and
// the switch only), like the other per-agent permission rows.
export function RemoteInstructToggle({
  project,
  compact = false,
  readOnly = false,
  fetchProjects,
}: {
  project: ProjectWithWorkspaces
  /** Drawer Agent tab: the label and switch only (no description box). */
  compact?: boolean
  /** Shown but not flippable: a room on another server, or a role below the route floor. */
  readOnly?: boolean
  fetchProjects: () => Promise<void>
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const enabled = (project.allowRemoteInstruct ?? 0) === 1
  // With the master ON, the agent is listed and reachable even while this
  // toggle shows OFF. Surface that so an OFF toggle never reads as a deny
  // it doesn't enforce.
  const globalAllow = useSettingsStore((s) => s.allowRemoteInstruct)

  const toggle = async (): Promise<void> => {
    if (busy || readOnly) return
    const next = !enabled
    setBusy(true)
    try {
      await daemonCliPostQuery(primaryScope(), 'remote-instruct', {
        project: project.path,
        enable: next ? '1' : '0',
      })
      await fetchProjects()
    } catch (err) {
      console.error('[remote-instruct] write failed', err)
    } finally {
      setBusy(false)
    }
  }

  return (
    <div className={compact ? 'px-3 py-1.5' : 'border border-[var(--color-border)] p-3'}>
      <div className="flex items-start gap-3">
        <button
          onClick={toggle}
          role="switch"
          aria-checked={enabled}
          aria-label={AGENT_SWITCH_LABEL}
          disabled={busy || readOnly}
          data-settings-id="projects.allow-remote-instruct"
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
          title={
            enabled
              ? 'Agents on connected servers can message this agent'
              : "Agents on connected servers can't message this agent"
          }
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)]">{AGENT_SWITCH_LABEL}</div>
          <div className={compact ? 'hidden' : 'text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed'}>
            {enabled ? AGENT_SWITCH_ON_TEXT : AGENT_SWITCH_OFF_TEXT}
          </div>
          {!enabled && globalAllow && !readOnly && (
            <div className="text-[10px] text-[color-mix(in_srgb,var(--color-status-warn-amber-soft)_80%,transparent)] mt-1 leading-relaxed">
              {AGENT_SWITCH_OVERRIDE_TEXT}
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
