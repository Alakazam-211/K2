// Settings → Users and Access: what a Voice & Chat only login is shown
// (prd-voice-chat-only-members-v1, shares extension). The owner (or an
// Admin) names the agents whose Thread the person may read and post in,
// and the Gardens they may view on a phone. `POST /cli/users/set-shares
// {username, agents, gardens}` replaces both lists; the daemon resolves
// each agent address and Garden id and refuses an unknown one.

import { useEffect, useState } from 'react'
import { splitShareList, type VoiceChatShares } from './connect-user-roles'

const INPUT =
  'flex-1 min-w-0 px-2 py-1 text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'
const BUTTON =
  'flex-shrink-0 px-2 py-0.5 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-50'

export interface VoiceChatSharesEditorProps {
  username: string
  shares: VoiceChatShares
  /** Save both lists. Resolves to an error message, or null on success. */
  onSave: (agents: string[], gardens: string[]) => Promise<string | null>
}

function agentsText(s: VoiceChatShares): string {
  return s.agents.map((a) => a.address).join(', ')
}

function gardensText(s: VoiceChatShares): string {
  return s.gardens.map((g) => g.id).join(', ')
}

export function VoiceChatSharesEditor({ username, shares, onSave }: VoiceChatSharesEditorProps): React.JSX.Element {
  const [agents, setAgents] = useState(agentsText(shares))
  const [gardens, setGardens] = useState(gardensText(shares))
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const savedAgents = agentsText(shares)
  const savedGardens = gardensText(shares)
  // A refresh of the list (another window, the CLI) shows the new shares.
  useEffect(() => {
    setAgents(savedAgents)
    setGardens(savedGardens)
  }, [savedAgents, savedGardens])
  const dirty = agents !== savedAgents || gardens !== savedGardens
  const save = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    setSaved(false)
    const err = await onSave(splitShareList(agents), splitShareList(gardens))
    setBusy(false)
    if (err) setError(err)
    else setSaved(true)
  }
  return (
    <div className="pl-7 space-y-1" data-testid={`voice-chat-shares-${username}`}>
      <div className="text-[10px] text-[var(--color-text-muted)]">
        Shared with {username}: they can read and post in these agents&apos; Threads and view these Gardens. Nothing else.
      </div>
      <div className="flex items-center gap-2">
        <span className="w-16 flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">Agents</span>
        <input
          className={INPUT}
          value={agents}
          placeholder="agent address, e.g. demo"
          aria-label={`${username} shared agents`}
          onChange={(e) => setAgents(e.target.value)}
        />
      </div>
      <div className="flex items-center gap-2">
        <span className="w-16 flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">Gardens</span>
        <input
          className={INPUT}
          value={gardens}
          placeholder="Garden id, e.g. g-1a2b3c4d"
          aria-label={`${username} shared Gardens`}
          onChange={(e) => setGardens(e.target.value)}
        />
        <button type="button" className={BUTTON} disabled={busy || !dirty} onClick={() => void save()}>
          {busy ? 'Saving…' : 'Save shares'}
        </button>
      </div>
      {error ? <div className="text-[10px] text-[var(--color-status-error-soft)]">{error}</div> : null}
      {saved && !dirty ? <div className="text-[10px] text-[var(--color-text-muted)]">Saved</div> : null}
    </div>
  )
}
