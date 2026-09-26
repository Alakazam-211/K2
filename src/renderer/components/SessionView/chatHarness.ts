/** v1 chat face. Later harnesses are another adapter, not this set. */
export const CHAT_HARNESSES = ['claude', 'codex', 'grok', 'gemini'] as const

export type ChatHarness = (typeof CHAT_HARNESSES)[number]

const CHAT_HARNESS_SET = new Set<string>(CHAT_HARNESSES)

/** First-token basename. `~/bin/claude` and `claude --resume` are `claude`. */
export function commandBasename(command?: string | null): string {
  if (!command) return ''
  const first = command.trim().split(/\s+/)[0] ?? ''
  return first.split(/[/\\]/).pop() ?? ''
}

/**
 * Spawn basename, including a restored tab that only kept `commandHint`.
 * A known non-v1 provider (`pi`, `cursor`) does not fall through to the hint.
 * Null program / empty fields are not eligible. Does not probe the foreground.
 */
export function chatHarnessName(input: {
  command?: string | null
  commandHint?: string | null
  provider?: string | null
}): ChatHarness | null {
  const provider = input.provider?.trim()
  if (provider) {
    return CHAT_HARNESS_SET.has(provider) ? (provider as ChatHarness) : null
  }
  const fromCmd = commandBasename(input.command)
  if (CHAT_HARNESS_SET.has(fromCmd)) return fromCmd as ChatHarness
  const fromHint = commandBasename(input.commandHint)
  if (CHAT_HARNESS_SET.has(fromHint)) return fromHint as ChatHarness
  return null
}

export function harnessFieldsKnown(input: {
  command?: string | null
  commandHint?: string | null
  provider?: string | null
}): boolean {
  return Boolean(input.provider?.trim() || input.command?.trim() || input.commandHint?.trim())
}
