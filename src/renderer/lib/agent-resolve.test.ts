import { describe, it, expect } from 'vitest'
import {
  parseCommand,
  matchAgentPreset,
  resolveAgentPreset,
  resolveAgentCommand,
  readProjectDefaultAgent,
  type AgentPresetLike,
} from './agent-resolve'

// Mirrors the daemon-seeded agent_presets shape (fixed UUIDs, enabled 0/1).
const CLAUDE_ID = '11111111-1111-4111-8111-111111111111'
const CODEX_ID = '22222222-2222-4222-8222-222222222222'
const PI_ID = '33333333-3333-4333-8333-333333333333'
const GROK_ID = '44444444-4444-4444-8444-444444444444'

const presets: AgentPresetLike[] = [
  { id: CLAUDE_ID, label: 'Claude Code', command: 'claude --dangerously-skip-permissions', enabled: 1 },
  { id: CODEX_ID, label: 'Codex', command: 'codex', enabled: 1 },
  { id: PI_ID, label: 'Pi', command: 'pi', enabled: 1 },
  { id: GROK_ID, label: 'Grok', command: 'grok', enabled: 0 }, // disabled
]

describe('resolveAgentPreset', () => {
  it('matches the stored value by preset id (canonical representation)', () => {
    const p = resolveAgentPreset(presets, CODEX_ID)
    expect(p).not.toBeNull()
    expect(p!.id).toBe(CODEX_ID)
  })

  it('matches legacy command-token values (pre-Slice-0 settings.json)', () => {
    const p = resolveAgentPreset(presets, 'codex')
    expect(p).not.toBeNull()
    expect(p!.id).toBe(CODEX_ID)
  })

  it('matches a legacy token against the command FIRST token, not the whole command', () => {
    const p = resolveAgentPreset(presets, 'claude')
    expect(p).not.toBeNull()
    expect(p!.id).toBe(CLAUDE_ID)
  })

  it('prefers the workspace value over the global value', () => {
    const p = resolveAgentPreset(presets, CLAUDE_ID, PI_ID)
    expect(p!.id).toBe(PI_ID)
  })

  it('prefers a legacy-token workspace value over an id global value', () => {
    const p = resolveAgentPreset(presets, CLAUDE_ID, 'pi')
    expect(p!.id).toBe(PI_ID)
  })

  it('falls through an unmatchable workspace value to the global value', () => {
    const p = resolveAgentPreset(presets, CODEX_ID, 'no-such-agent')
    expect(p!.id).toBe(CODEX_ID)
  })

  it('falls back to the first enabled preset when neither value matches', () => {
    const p = resolveAgentPreset(presets, 'no-such-agent')
    expect(p!.id).toBe(CLAUDE_ID)
  })

  it('falls back to the first enabled preset when the value is empty/undefined', () => {
    expect(resolveAgentPreset(presets, undefined)!.id).toBe(CLAUDE_ID)
    expect(resolveAgentPreset(presets, null)!.id).toBe(CLAUDE_ID)
    expect(resolveAgentPreset(presets, '')!.id).toBe(CLAUDE_ID)
  })

  it('never returns a disabled preset, even on exact id match', () => {
    const p = resolveAgentPreset(presets, GROK_ID)
    expect(p!.id).not.toBe(GROK_ID)
    expect(p!.id).toBe(CLAUDE_ID) // fallback
  })

  it('never returns a disabled preset on legacy-token match', () => {
    const p = resolveAgentPreset(presets, 'grok')
    expect(p!.id).toBe(CLAUDE_ID) // fallback
  })

  it('skips disabled presets when falling back to "first enabled"', () => {
    const disabledFirst: AgentPresetLike[] = [
      { id: GROK_ID, label: 'Grok', command: 'grok', enabled: 0 },
      { id: PI_ID, label: 'Pi', command: 'pi', enabled: 1 },
    ]
    expect(resolveAgentPreset(disabledFirst, 'nope')!.id).toBe(PI_ID)
  })

  it('returns null when no preset is enabled', () => {
    const allDisabled: AgentPresetLike[] = [
      { id: GROK_ID, label: 'Grok', command: 'grok', enabled: 0 },
    ]
    expect(resolveAgentPreset(allDisabled, GROK_ID)).toBeNull()
    expect(resolveAgentPreset([], 'claude')).toBeNull()
  })
})

describe('resolveAgentCommand', () => {
  it('returns the preset plus parsed {command, args}', () => {
    const r = resolveAgentCommand(presets, 'claude')
    expect(r).not.toBeNull()
    expect(r!.preset.id).toBe(CLAUDE_ID)
    expect(r!.command).toBe('claude')
    expect(r!.args).toEqual(['--dangerously-skip-permissions'])
  })

  it('honors workspace-over-global precedence end to end', () => {
    const r = resolveAgentCommand(presets, CLAUDE_ID, CODEX_ID)
    expect(r!.command).toBe('codex')
    expect(r!.args).toEqual([])
  })

  it('returns null when nothing is resolvable', () => {
    expect(resolveAgentCommand([], 'claude')).toBeNull()
  })
})

describe('matchAgentPreset (display matching — no fallback, no enabled filter)', () => {
  it('matches by id, then token, else null', () => {
    expect(matchAgentPreset(presets, PI_ID)!.id).toBe(PI_ID)
    expect(matchAgentPreset(presets, 'pi')!.id).toBe(PI_ID)
    expect(matchAgentPreset(presets, 'nope')).toBeNull()
    expect(matchAgentPreset(presets, undefined)).toBeNull()
  })

  it('does match disabled presets (display concern, not launch)', () => {
    expect(matchAgentPreset(presets, GROK_ID)!.id).toBe(GROK_ID)
  })
})

describe('parseCommand', () => {
  it('splits command and args', () => {
    expect(parseCommand('claude --resume abc')).toEqual({
      command: 'claude',
      args: ['--resume', 'abc'],
    })
  })

  it('respects quoted segments', () => {
    expect(parseCommand('claude --append-system-prompt "be nice"')).toEqual({
      command: 'claude',
      args: ['--append-system-prompt', 'be nice'],
    })
  })

  it('handles the empty string', () => {
    expect(parseCommand('')).toEqual({ command: '', args: [] })
  })
})

describe('readProjectDefaultAgent (Slice-1 forward compatibility)', () => {
  it('is undefined-safe for projects without the field (today)', () => {
    expect(readProjectDefaultAgent({ id: 'p1', name: 'K2' })).toBeUndefined()
    expect(readProjectDefaultAgent(undefined)).toBeUndefined()
    expect(readProjectDefaultAgent(null)).toBeUndefined()
  })

  it('reads camelCase and snake_case spellings once Slice 1 lands', () => {
    expect(readProjectDefaultAgent({ defaultAgent: CODEX_ID })).toBe(CODEX_ID)
    expect(readProjectDefaultAgent({ default_agent: 'pi' })).toBe('pi')
  })

  it('rejects empty/non-string values', () => {
    expect(readProjectDefaultAgent({ defaultAgent: '' })).toBeUndefined()
    expect(readProjectDefaultAgent({ defaultAgent: 42 })).toBeUndefined()
    expect(readProjectDefaultAgent({ defaultAgent: null })).toBeUndefined()
  })
})
