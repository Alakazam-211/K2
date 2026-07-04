/**
 * Provider-id → icon bridge for chat-session rows.
 *
 * Discovery surfaces (`chat/list` → ChatHistory browser, AgentChatPane's
 * canonical-session dropdown) identify sessions by PROVIDER id ("claude",
 * "pi", "cursor", …) while `<AgentIcon>` matches on agent/preset NAMES
 * ("Claude", "Cursor Agent", …). This module owns the one provider→
 * agent-name mapping so every session list renders the same mark for the
 * same provider. Extracted from ChatHistory.tsx (agent-degeneralization
 * Slice 4) so the pinned-tab dropdown reuses it instead of duplicating.
 */

import AgentIcon from './AgentIcon'

/** Map chat-history provider ids to AgentIcon agent names. Unknown
 *  providers fall through to the raw id — AgentIcon substring-matches,
 *  so a future provider whose id matches its brand still gets a mark. */
export const PROVIDER_AGENT_NAME: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor Agent',
  gemini: 'Gemini',
  pi: 'Pi',
  codex: 'Codex',
  grok: 'Grok',
  hermes: 'Hermes',
}

interface ProviderIconProps {
  provider: string
  size?: number
}

export function ProviderIcon({ provider, size = 14 }: ProviderIconProps): React.JSX.Element {
  const agentName = PROVIDER_AGENT_NAME[provider] ?? provider
  return <AgentIcon agent={agentName} size={size} />
}
