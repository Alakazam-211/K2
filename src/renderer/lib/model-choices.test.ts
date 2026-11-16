import { describe, expect, it } from 'vitest'
import { checkedModelRow, defaultModelHarness, modelPickerRows, modelSuggestionsForAgent, showModelChip } from './model-choices'

describe('model choices (0.46.13)', () => {
  it('lists only this harness’s models', () => {
    expect(modelSuggestionsForAgent('/usr/local/bin/claude --dangerously-skip-permissions')).toEqual(['opus', 'sonnet', 'haiku'])
    expect(modelSuggestionsForAgent('codex --yolo')).toContain('gpt-5.3-codex')
    expect(modelSuggestionsForAgent('bash')).toEqual([])
    const rows = modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: 'opus' })
    expect(rows.map((r) => r.value)).toEqual(['', 'opus', 'sonnet', 'haiku'])
    expect(rows.some((r) => r.value.startsWith('gpt'))).toBe(false)
  })

  it('the Thread’s first row names what the server default is now', () => {
    const [first] = modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: 'opus' })
    expect(first).toEqual({ value: '', label: 'Server default (Opus 5.5)', secondary: 'Set in Settings → Providers / Models' })
    expect(modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: null })[0].label).toBe('Server default (harness picks)')
  })

  it('Settings has no server-default row: unset reads Harness default', () => {
    const rows = modelPickerRows({ harness: 'claude', mode: 'setting' })
    expect(rows[0].label).toBe('Harness default')
    expect(rows.some((r) => r.label.startsWith('Server default'))).toBe(false)
  })

  it('filters by name and id, and keeps a current model that isn’t a suggestion', () => {
    expect(modelPickerRows({ harness: 'claude', mode: 'thread', query: 'op' }).map((r) => r.value)).toEqual(['opus'])
    expect(modelPickerRows({ harness: 'claude', mode: 'thread', query: 'HAIKU 5' }).map((r) => r.value)).toEqual(['haiku'])
    expect(modelPickerRows({ harness: 'claude', mode: 'thread', query: 'xyz' })).toEqual([])
    expect(modelPickerRows({ harness: 'claude', mode: 'thread', current: 'claude-opus-5-5' }).map((r) => r.value)).toContain('claude-opus-5-5')
  })

  it('checks the current model, else the default row', () => {
    const rows = modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: 'opus', current: 'haiku' })
    expect(checkedModelRow(rows, 'haiku', 'opus')).toBe('haiku')
    expect(checkedModelRow(rows, 'opus', 'opus')).toBe('')
    expect(checkedModelRow(rows, null, 'opus')).toBe('')
  })

  it('the chip shows only off the server default', () => {
    expect(showModelChip('haiku', 'opus')).toBe(true)
    expect(showModelChip('opus', 'opus')).toBe(false)
    expect(showModelChip(null, 'opus')).toBe(false)
    expect(showModelChip('opus', null)).toBe(true)
  })

  it('maps CLI names to the settings keys', () => {
    expect(defaultModelHarness('claude')).toBe('claude')
    expect(defaultModelHarness('agent')).toBe('cursor-agent')
    expect(defaultModelHarness('aider')).toBeNull()
  })
})
