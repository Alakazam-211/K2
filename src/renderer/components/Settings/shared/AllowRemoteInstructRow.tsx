import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'
import { Toggle } from '@/components/ui'

/** App master label (Rosson 2026-10-16). */
export const MASTER_SWITCH_LABEL = 'Let agents on other servers message every agent'
/** Shown while the master is on. */
export const MASTER_SWITCH_ON_TEXT =
  "On: every agent here is listed to connected servers and their agents can message it, whatever each agent's own switch says. People who log in here can always message agents."
/** Shown while the master is off. */
export const MASTER_SWITCH_OFF_TEXT =
  'Off: only agents whose own "Let agents on other servers message this agent" switch is on are listed to connected servers, and only they can be messaged by their agents. People who log in here can always message agents.'

// ── App master: "Let agents on other servers message every agent" ──────
// Internal name: `allowRemoteInstruct` in `~/.k2/settings.json`. Since
// 2026-10-16 (Rosson) it covers FEDERATED agents only: on → every agent on
// this server is in the roster paired servers see and federated live
// messages to it are delivered; off → only agents whose own switch
// (Workspaces → the workspace → "Let agents on other servers message this
// agent") is on. People who log in here (owner, Admins, Members) can always
// message agents; their role decides it on the daemon, not this switch.
//
// Home: Settings → K2 Connect, directly beneath "Enable federation".
export function AllowRemoteInstructRow(): React.JSX.Element {
  const allow = useSettingsStore((s) => s.allowRemoteInstruct)
  const setAllow = useSettingsStore((s) => s.setAllowRemoteInstruct)

  const toggle = useCallback(() => {
    void setAllow(!allow)
  }, [allow, setAllow])

  return (
    <div className="flex items-center justify-between py-2" data-settings-id="k2-connect.allow-remote-instruct">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">{MASTER_SWITCH_LABEL}</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {allow ? MASTER_SWITCH_ON_TEXT : MASTER_SWITCH_OFF_TEXT}
        </p>
      </div>
      <Toggle checked={allow} onChange={toggle} aria-label={MASTER_SWITCH_LABEL} />
    </div>
  )
}
