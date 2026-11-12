// "Allow a virtual desktop" for one agent (prd-linux-virtual-desktop-v1
// LD-38, migration 0301; off by default). On: the agent may ask for a
// desktop of its own on this Linux server (`k2 desktop env`), put windows on
// it, and keep it awake while it works; people watch it in a Desktop tab.
// Shown only on a server with virtual desktops (`virtual-desktop-v1`: Linux).
// The Owner or an Admin flips it, both ways; a Member never sees it.
//
// Reads GET /cli/desktop/status?workspace=<project id> (`agentsAllowed`);
// writes POST /cli/agent-access/set {toggle: 'desktop', workspace, value}.
// The daemon enforces who may flip it.

import { useCallback, useEffect, useState } from 'react'
import { BetaTag, Toggle } from '@/components/ui'
import { fetchViewerRole } from '@/components/Presence/PresenceKickButton'
import { VIRTUAL_DESKTOP_FEATURE } from '@/components/Desktop/desktop-state'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import type { ServerScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'
import { AskedChip } from './PermissionCardsSection'

export const VIRTUAL_DESKTOP_ROW = 'projects.agents-can-use-desktop'
export const VIRTUAL_DESKTOP_LABEL = 'Allow a virtual desktop'

export function AgentsVirtualDesktopToggle({
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
  const supported = scope.serverSupports(VIRTUAL_DESKTOP_FEATURE)
  const [role, setRole] = useState<'owner' | 'admin' | 'member' | null>(null)
  const [on, setOn] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = useSaveStatus()

  const load = useCallback(async (): Promise<void> => {
    try {
      const v = (await daemonCliGet(scope, 'desktop/status', { workspace: project.id })) as { agentsAllowed?: unknown }
      setOn(v.agentsAllowed === true)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }, [scope, project.id])

  useEffect(() => {
    if (!supported) return
    let live = true
    void fetchViewerRole().then((r) => {
      if (live) setRole(r as 'owner' | 'admin' | 'member' | null)
    })
    void load()
    return () => {
      live = false
    }
  }, [supported, load])

  if (!supported || role === 'member') return null
  const canFlip = role === 'owner' || role === 'admin'

  const toggle = async (): Promise<void> => {
    if (busy || !canFlip || readOnly || on === null) return
    const next = !on
    setBusy(true)
    setError(null)
    setOn(next) // optimistic
    try {
      await daemonCliPost<unknown>(scope, 'agent-access/set', { toggle: 'desktop', workspace: project.path, value: next })
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
            aria-label={VIRTUAL_DESKTOP_LABEL}
            disabled={busy || !canFlip || readOnly || on === null}
            data-settings-id={VIRTUAL_DESKTOP_ROW}
            data-testid="agents-virtual-desktop"
            title={
              role === null || on === null
                ? 'Checking this server'
                : on
                  ? 'This agent may have a desktop of its own on this server'
                  : "This agent can't have a desktop"
            }
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>
        {VIRTUAL_DESKTOP_LABEL}
        <BetaTag className="ml-2" />
        <AskedChip row={VIRTUAL_DESKTOP_ROW} />
      </div>
      <div className={compact ? 'hidden' : SETTING_ROW_HINT}>
        It can start a desktop of its own on this server and put windows on it, such as a browser, so you can watch and
        help in a Desktop tab. Separate, not private: other agents on this server could still reach it.
      </div>
      {error ? (
        <div className="type-hint text-[var(--color-status-error-soft)] mt-1" data-testid="agents-virtual-desktop-error">
          {error}
        </div>
      ) : null}
    </SettingSwitchRow>
  )
}
