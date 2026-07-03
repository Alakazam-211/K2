// Agent de-generalization Slice 0 — the ONE renderer seam that turns the
// `defaultAgent` setting (+ future per-workspace override) into a concrete
// agent preset / spawn command.
//
// History: the Editors & Agents picker used to store the command's FIRST
// TOKEN ("claude") while ~10 consumers matched `p.id === defaultAgent`
// (a preset UUID) — never true, so they silently fell back to the first
// enabled preset. The setting only appeared to work because Claude was both
// the default and the fallback. The canonical stored value is now the
// preset id (`agent_presets` UUID), but resolution stays TOLERANT: legacy
// settings.json values like "claude" keep working via command-token match.
// No data migration.
//
// This module is deliberately PURE (zero imports) so stores (tabs.ts,
// active-agents.ts) can use it without import cycles and tests need no
// zustand/tauri scaffolding. React consumers should prefer the thin wrapper
// hook `useResolvedAgentCommand` (src/renderer/hooks/useResolvedAgentCommand.ts).

/** Minimum shape needed to MATCH a stored value against a preset. */
export interface AgentPresetRef {
  id: string
  command: string
}

/** Structural subset of `AgentPreset` (stores/presets.ts) this seam needs. */
export interface AgentPresetLike extends AgentPresetRef {
  label: string
  /** Daemon rows use 0/1; accept booleans for callers/tests. */
  enabled: number | boolean
}

export interface ResolvedAgentCommand<P extends AgentPresetLike = AgentPresetLike> {
  preset: P
  /** First token of the preset command (e.g. "claude"). */
  command: string
  /** Remaining preset args (quote-aware split). */
  args: string[]
}

/**
 * Split a preset command string into command + args, respecting quotes.
 * (Moved verbatim from stores/presets.ts so this module stays store-free;
 * stores/presets.ts re-exports it for existing importers.)
 */
export function parseCommand(commandStr: string): { command: string; args: string[] } {
  // Split respecting quoted strings
  const parts: string[] = []
  let current = ''
  let inQuote: string | null = null

  for (let i = 0; i < commandStr.length; i++) {
    const ch = commandStr[i]

    if (inQuote) {
      if (ch === inQuote) {
        inQuote = null
      } else {
        current += ch
      }
    } else if (ch === '"' || ch === "'") {
      inQuote = ch
    } else if (ch === ' ') {
      if (current.length > 0) {
        parts.push(current)
        current = ''
      }
    } else {
      current += ch
    }
  }
  if (current.length > 0) {
    parts.push(current)
  }

  const [command, ...args] = parts
  return { command: command || '', args }
}

/** First whitespace-delimited token of a preset's command string. */
function commandFirstToken(preset: AgentPresetRef): string {
  return preset.command.split(/\s+/)[0] ?? ''
}

/**
 * Match a stored default-agent VALUE against the preset list — by preset id
 * first (canonical), then by command first-token (legacy "claude"-style
 * values). No enabled filter, no fallback: returns exactly what the value
 * names, or null. Use this for DISPLAY (e.g. the settings picker); use
 * `resolveAgentPreset` when you need something launchable.
 */
export function matchAgentPreset<P extends AgentPresetRef>(
  presets: readonly P[],
  value: string | null | undefined,
): P | null {
  if (!value) return null
  return (
    presets.find((p) => p.id === value) ??
    presets.find((p) => commandFirstToken(p) === value) ??
    null
  )
}

/**
 * Resolve the effective default-agent preset.
 *
 * Precedence: `workspaceDefaultValue` (per-workspace override — Slice 1)
 * → `defaultAgentValue` (global setting) → first enabled preset. Each value
 * is matched by preset id first, then by command first-token (legacy).
 * Disabled presets are never returned. Returns null only when NO preset is
 * enabled (e.g. presets not yet fetched).
 */
export function resolveAgentPreset<P extends AgentPresetLike>(
  presets: readonly P[],
  defaultAgentValue: string | null | undefined,
  workspaceDefaultValue?: string | null,
): P | null {
  const enabled = presets.filter((p) => !!p.enabled)
  return (
    matchAgentPreset(enabled, workspaceDefaultValue) ??
    matchAgentPreset(enabled, defaultAgentValue) ??
    enabled[0] ??
    null
  )
}

/**
 * `resolveAgentPreset` + `parseCommand` in one step — what launch sites
 * actually want: `{ preset, command, args }` ready for addTab/spawn. Per-
 * agent behavior stays at the call site (e.g. gate --append-system-prompt
 * on `resolved.command === 'claude'`); this seam centralizes RESOLUTION only.
 */
export function resolveAgentCommand<P extends AgentPresetLike>(
  presets: readonly P[],
  defaultAgentValue: string | null | undefined,
  workspaceDefaultValue?: string | null,
): ResolvedAgentCommand<P> | null {
  const preset = resolveAgentPreset(presets, defaultAgentValue, workspaceDefaultValue)
  if (!preset) return null
  const { command, args } = parseCommand(preset.command)
  return { preset, command, args }
}

/**
 * Tolerantly read a project's per-workspace default-agent value. The field
 * does not exist yet (Slice 1 adds `projects.default_agent`); this is
 * undefined-safe today and picks the field up automatically when it lands
 * (accepts both camelCase and snake_case spellings, non-empty strings only).
 */
export function readProjectDefaultAgent(project: unknown): string | undefined {
  if (typeof project !== 'object' || project === null) return undefined
  const rec = project as { defaultAgent?: unknown; default_agent?: unknown }
  const raw = rec.defaultAgent ?? rec.default_agent
  return typeof raw === 'string' && raw.length > 0 ? raw : undefined
}
