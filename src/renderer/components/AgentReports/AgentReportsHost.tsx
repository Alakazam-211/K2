// The agent-reports card, once per window (plan-agent-reports-opt-in-app-v1).
// It opens from the setup list's row or Settings' "Choose now"
// (`openAgentReportsCard`), and by itself while the daemon offers it to this
// person (`offers: ["agent_reports"]`: nobody has answered, they are the
// owner, an Owner or an Admin, their setup list isn't in progress, and they
// haven't said "Ask me later"). The daemon decides; this draws it.

import { useEffect, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { useFirstRunStore, watchFirstRun } from '@/stores/first-run'
import { useAgentReportsCardStore } from '@/stores/agent-reports-card'
import { AgentReportsCard } from './AgentReportsCard'

export function AgentReportsHost(): React.JSX.Element | null {
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  const openAs = useAgentReportsCardStore((s) => s.openAs)
  const close = useAgentReportsCardStore((s) => s.close)
  const offered = useFirstRunStore((s) => s.entries[scope.id]?.doc?.offers?.includes('agent_reports') === true)
  // Done with the offer in this window (saved or later): don't draw it
  // again while the daemon's next answer is on its way.
  const [offerDone, setOfferDone] = useState<string | null>(null)

  const kind = openAs ?? (offered && offerDone !== scope.id ? 'offer' : null)
  if (!kind) return null
  return (
    <AgentReportsCard
      key={`${scope.id}:${kind}`}
      scope={scope}
      kind={kind}
      onClose={() => {
        if (kind === 'offer') setOfferDone(scope.id)
        else close()
      }}
    />
  )
}
