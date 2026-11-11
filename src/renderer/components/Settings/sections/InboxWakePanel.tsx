// The inbox's own panel in the email areas (hosted and linked): for each
// agent that can read this inbox, does it wake that agent? The same record as
// the agent's Mail tab (`/cli/mail/watch`), so the two can't disagree. The
// default for every agent is Off: nothing wakes anyone until someone chooses.

import React, { useCallback, useEffect, useState } from 'react'
import { SettingDropdown } from '../controls/SettingControls'
import { mailErrorMessage, onWindowServerMailChanged, type Inbox } from './email-api'
import {
  fetchInboxWatchers,
  setWatchInbox,
  wakeFromValue,
  wakeValue,
  WAKE_OPTIONS,
  type InboxWake,
} from './mail-watch-api'

const muted = 'type-hint text-[var(--color-text-secondary)]'

/** Each workspace that can read this inbox (the Primary and its grants). */
function readers(inbox: Inbox): { projectId: string; name: string }[] {
  return [inbox.primary, ...inbox.grants].map((p) => ({ projectId: p.projectId, name: p.workspace ?? p.projectId }))
}

export function InboxWakePanel({ inbox, canMutate }: { inbox: Inbox; canMutate: boolean }): React.JSX.Element | null {
  const [wakes, setWakes] = useState<Record<string, InboxWake> | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const { agents } = await fetchInboxWatchers(inbox.address)
      setWakes(Object.fromEntries(agents.map((a) => [a.projectId, a.wake])))
      setError(null)
    } catch (e) {
      setError(mailErrorMessage(e))
    }
  }, [inbox.address])

  useEffect(() => {
    void load()
    return onWindowServerMailChanged(() => void load())
  }, [load])

  const people = readers(inbox)
  if (people.length === 0) return null

  const change = async (projectId: string, value: string): Promise<void> => {
    try {
      await setWatchInbox(projectId, inbox.address, wakeFromValue(value))
      await load()
    } catch (e) {
      setError(mailErrorMessage(e))
    }
  }

  return (
    <div className="pt-2 space-y-1">
      <div className="type-row text-[var(--color-text-primary)]">Wakes these agents</div>
      <div className={muted}>Whether new mail here wakes each agent. Off until someone chooses. VIP senders are the people whose mail should reach an agent right away; set them on the agent’s Mail tab.</div>
      {error && <div data-wake-error="" className="type-hint text-[var(--color-status-error-soft)]">Couldn’t read which agents this address wakes. <span className="text-[var(--color-text-muted)]">The server said: {error.trim().replace(/[.\s]+$/, '')}.</span></div>}
      {people.map((p) => (
        <div key={p.projectId} className="flex items-center justify-between gap-2 py-0.5">
          <span className="type-row text-[var(--color-text-primary)] truncate">{p.name}</span>
          <SettingDropdown
            value={wakeValue(wakes?.[p.projectId] ?? null)}
            options={WAKE_OPTIONS}
            disabled={!canMutate || wakes === null}
            ariaLabel={`How this inbox wakes ${p.name}`}
            onChange={(v) => change(p.projectId, v)}
          />
        </div>
      ))}
    </div>
  )
}
