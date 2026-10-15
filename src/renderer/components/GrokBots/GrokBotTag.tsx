// The "Grok Bot" tag on every Grok Bot row and chat header (0.45.7), so a
// Grok Bot is never mistaken for a K2 agent.

import { GROK_BOT_LABEL } from '@/lib/grokbots'

export function GrokBotTag({ className = '' }: { className?: string }): React.JSX.Element {
  return (
    <span
      data-testid="grokbot-tag"
      title="A Grok Bot agent on this server's computer (not a K2 workspace)"
      className={`inline-flex shrink-0 items-center rounded-[3px] border border-[var(--color-border)] px-1 text-[9px] font-medium uppercase leading-[14px] tracking-wide text-[var(--color-text-muted)] ${className}`.trim()}
    >
      {GROK_BOT_LABEL}
    </span>
  )
}
