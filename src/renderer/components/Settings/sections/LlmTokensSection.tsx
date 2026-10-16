// Settings → Language Models → Subs / Tokens (was LLMs → Tokens; 0.45.7, Rosson: "Group the LLM details under
// one section as well: LLMs · Tokens · Token Usage"). The page around
// `AgentAccountsColumn`: this server's subscriptions and API tokens, the
// server default per tool, switching, Use next token, Sign in again, each
// token's usage and when it refreshes. Before 0.45.7 the same column was
// the right half of Settings → LLMs; it shows exactly what it did there.
//
// LLMs (default agent, launch bar, presets, CLI install) stays on the
// `agents` page, the group's first leaf, so `openSettings('agents')` still
// lands on the LLMs group. Token Usage (`token-usage`) is the third leaf.
// The search ids (`agents.accounts`, `agents.add-login`) and the
// `data-settings-id`s on the column are unchanged.

import React from 'react'
import type { SettingEntry } from '../searchManifest'
import { AgentAccountsColumn } from '../shared/AgentAccountsColumn'

export const LLM_TOKENS_MANIFEST: SettingEntry[] = [
  {
    id: 'agents.accounts',
    section: 'llm-tokens',
    label: 'Subs / Tokens',
    description: 'Which subscription or API token each agent CLI uses on this server; add, switch or remove tokens',
    keywords: ['token', 'tokens', 'subscription', 'subscriptions', 'llm', 'llms', 'language models', 'api token', 'api key', 'server default', 'account', 'login', 'sign in', 'switch', 'claude', 'codex', 'grok'],
  },
  {
    id: 'agents.add-login',
    section: 'llm-tokens',
    label: 'Add subscription or API token',
    description: 'Sign another Claude, Codex or Grok subscription in, or paste an API token, on this server',
    keywords: ['add', 'token', 'subscription', 'tokens', 'llm', 'language models', 'api token', 'api key', 'account', 'login', 'sign in', 'claude', 'codex', 'grok', 'gemini'],
  },
]

export function LlmTokensSection(): React.JSX.Element {
  return (
    <div className="max-w-3xl" data-llm-tokens-section="">
      <AgentAccountsColumn />
    </div>
  )
}
