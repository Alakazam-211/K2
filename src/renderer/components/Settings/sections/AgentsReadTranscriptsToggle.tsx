// "Read call transcripts" for one agent (K2 0.46.11; Rosson 2026-11-10: off
// by default, the Owner turns it on). On: the agent lists this server's
// recorded voice-room calls and reads their transcripts (`k2 voice
// recordings`, `k2 voice transcript`); every read is logged, and while any
// agent has it each person's recording notice says "Agents on this server may
// read the transcript." An App's own calls are not included. The Owner turns
// it on; the Owner or an Admin turns it off; a Member never sees it. Shown
// only while voice rooms are on.
//
// Writes POST /cli/agent-access/set {toggle: 'voice_transcripts', workspace,
// value}; reads GET /cli/voice/transcripts (`readerIds`). The daemon enforces
// who may flip it.

import { useCallback, useEffect, useState } from 'react'
import { BetaTag, Toggle } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { useSettingsStore } from '@/stores/settings'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'
import { AskedChip } from './PermissionCardsSection'
import { manageOrgChartRights } from './AgentsManageOrgChartToggle'

export const READ_TRANSCRIPTS_ROW = 'projects.agents-can-read-transcripts'
export const READ_TRANSCRIPTS_LABEL = 'Read call transcripts'

export function AgentsReadTranscriptsToggle({
  project,
  scope,
  compact = false,
  readOnly = false,
}: {
  project: ProjectWithWorkspaces
  scope: ServerScope
  compact?: boolean
  readOnly?: boolean
}): React.JSX.Element | null {
  const voiceOn = useSettingsStore((s) => s.voiceRoomsPreview) === true
  const [role, setRole] = useState<'owner' | 'admin' | 'member' | null>(null)
  const [on, setOn] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = useSaveStatus()

  const load = useCallback(async (): Promise<void> => {
    try {
      const v = (await daemonCliGet(scope, 'voice/transcripts')) as { readerIds?: unknown }
      setOn(Array.isArray(v.readerIds) && v.readerIds.includes(project.id))
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [scope, project.id])

  useEffect(() => {
    if (!voiceOn) return
    let live = true
    void fetchViewerRole().then((r) => {
      if (live) setRole(r)
    })
    void load()
    return () => {
      live = false
    }
  }, [voiceOn, load])

  if (!voiceOn) return null
  // The same rule as "Manage the org chart": the Owner raises, an Admin lowers.
  const { show, canFlip } = manageOrgChartRights(role, on === true)
  if (!show) return null

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly || on === null) return
    const next = !on
    setBusy(true)
    setError(null)
    setOn(next) // optimistic
    try {
      await daemonCliPost<unknown>(scope, 'agent-access/set', { toggle: 'voice_transcripts', workspace: project.path, value: next })
      save.saved()
    } catch (e) {
      setOn(!next)
      setError(daemonErrorText(e))
      save.reset()
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      compact={compact}
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle
            size={compact ? 'sm' : 'md'}
            checked={on === true}
            onChange={toggle}
            aria-label={READ_TRANSCRIPTS_LABEL}
            disabled={busy || !canFlip || readOnly || on === null}
            data-settings-id={READ_TRANSCRIPTS_ROW}
            data-testid="agents-read-transcripts"
            title={
              role === null || on === null
                ? 'Checking this server'
                : !canFlip
                  ? "Only the server's owner turns this on"
                  : on
                    ? "This agent reads this server's call transcripts (every read is logged)"
                    : "This agent can't read call transcripts"
            }
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>
        {READ_TRANSCRIPTS_LABEL}
        <BetaTag className="ml-2" />
        <AskedChip row={READ_TRANSCRIPTS_ROW} />
      </div>
      <div className={compact ? 'hidden' : SETTING_ROW_HINT}>
        It can read the transcripts of recorded calls in this server&apos;s voice rooms. Every read is logged, and everyone&apos;s
        recording notice will say agents may read the transcript. An App&apos;s own calls are not included. Only the owner turns it on.
      </div>
      {error ? (
        <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-read-transcripts-error">
          {error}
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}
