// An agent row's activity counts (0.45.2): live subagents, and this turn's
// tool calls and the shell commands among them. Numbers only: no tool
// names or text ever reach a widget.

export interface ZenAgentCounts {
  subagents: number
  tools: number
  commands: number
}

/** "2 subagents · 14 tools · 5 commands", zero parts left out; null when
 *  there are no counts or all three are zero. */
export function zenCountsText(c: ZenAgentCounts | null | undefined): string | null {
  if (!c) return null
  const part = (n: number, one: string): string | null => (n > 0 ? `${n} ${one}${n === 1 ? '' : 's'}` : null)
  const parts = [part(c.subagents, 'subagent'), part(c.tools, 'tool'), part(c.commands, 'command')].filter(
    (p): p is string => p !== null,
  )
  return parts.length > 0 ? parts.join(' · ') : null
}
