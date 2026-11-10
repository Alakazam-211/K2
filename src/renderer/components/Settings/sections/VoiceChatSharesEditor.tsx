// Settings → Users and Access: what a Voice & Chat only login is shown
// (prd-voice-chat-only-members-v1, shares extension). The owner (or an
// Admin) names the agents the person gets their own conversation with,
// and the Gardens they may view on a phone. `POST /cli/users/set-shares
// {username, agents, gardens}` replaces both lists; the daemon resolves
// each agent address and Garden id and refuses an unknown one.

import { useEffect, useState } from 'react'
import { SHARED_CONTEXT_NOTICE, splitShareList, type VoiceChatShares } from './connect-user-roles'

import { Button, Input } from '@/components/ui'
import { SaveNote } from '../controls/SettingControls'

const INPUT = 'flex-1 min-w-0 type-control'

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
    <div className="mt-2 space-y-1.5" data-testid={`voice-chat-shares-${username}`}>
      <div className="type-hint text-[var(--color-text-secondary)] leading-relaxed">
        Shared with {username}: they get their own conversation with each of these agents and can view these Gardens.
        Nothing else.
      </div>
      {agents.trim() ? (
        <div className="type-hint text-[var(--color-status-warn-soft)]" data-testid={`voice-chat-shares-notice-${username}`}>
          {SHARED_CONTEXT_NOTICE}
        </div>
      ) : null}
      <div className="flex items-center gap-2">
        <span className="w-16 flex-shrink-0 type-hint text-[var(--color-text-secondary)]">Agents</span>
        <Input
          className={INPUT}
          value={agents}
          placeholder="agent address, e.g. demo"
          aria-label={`${username} shared agents`}
          onChange={(e) => setAgents(e.target.value)}
        />
      </div>
      <div className="flex items-center gap-2">
        <span className="w-16 flex-shrink-0 type-hint text-[var(--color-text-secondary)]">Gardens</span>
        <Input
          className={INPUT}
          value={gardens}
          placeholder="Garden id, e.g. g-1a2b3c4d"
          aria-label={`${username} shared Gardens`}
          onChange={(e) => setGardens(e.target.value)}
        />
        <Button className="flex-shrink-0" disabled={busy || !dirty} onClick={() => void save()}>
          {busy ? 'Saving…' : 'Save shares'}
        </Button>
      </div>
      {error ? (
        <div role="alert" className="type-hint text-[var(--color-status-error-soft)]">
          Couldn’t save what {username} can use: {error.replace(/[.\s]+$/, '')}. Nothing changed; try again.
        </div>
      ) : null}
      {saved && !dirty ? <SaveNote state="saved" /> : null}
    </div>
  )
}
