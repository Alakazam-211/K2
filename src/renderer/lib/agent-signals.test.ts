// Slice 5 — per-agent WORKING_SIGNALS coverage. Fixture strings are
// VERBATIM captures from the 2026-07 TUI signal studies
// (.k2/notes/tui-signal-study-grok-hermes-cursor.md /
//  .k2/notes/tui-signal-study-codex-gemini.md).

import { describe, it, expect } from 'vitest'
import {
  WORKING_SIGNALS,
  GROK_PERMISSION_TITLE_RE,
  detectWorkingSignal,
} from './agent-signals'

/** Build the `lines` map shape detectWorkingSignal consumes. */
function screen(rows: string[]): {
  lines: Map<number, { text: string }>
  total: number
} {
  const lines = new Map<number, { text: string }>()
  rows.forEach((text, i) => lines.set(i, { text }))
  return { lines, total: rows.length }
}

function detects(rows: string[]): boolean {
  const { lines, total } = screen(rows)
  return detectWorkingSignal(lines, total)
}

describe('WORKING_SIGNALS — per-agent verbatim captures', () => {
  it('claude/codex: "esc to interrupt" (codex confirmed verbatim)', () => {
    expect(detects(['• Working (0s • esc to interrupt)'])).toBe(true)
  })

  it('grok: busy footer "Esc:cancel │ Ctrl+.:shortcuts" (no space before colon)', () => {
    expect(detects(['Esc:cancel │ Ctrl+.:shortcuts'])).toBe(true)
  })

  it('grok: status row "⠙ Waiting for response… 0.0s" (via "waiting for ")', () => {
    expect(detects(['⠙ Waiting for response… 0.0s   [stop]'])).toBe(true)
  })

  it('grok: transcript "◆ Thinking…" (U+2026 — ASCII thinking... misses it)', () => {
    expect(detects(['◆ Thinking…'])).toBe(true)
  })

  it('grok: "Starting session…" (U+2026)', () => {
    expect(detects(['Starting session…'])).toBe(true)
  })

  it('hermes: busy footer "msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel"', () => {
    expect(
      detects(['msg=interrupt · /queue · /bg · /steer · Ctrl+C cancel']),
    ).toBe(true)
  })

  it('cursor-agent: mid-turn input bar "ctrl+c to stop"', () => {
    expect(detects(['⠰⠰ Working', 'ctrl+c to stop'])).toBe(true)
  })

  it('does not fire on an idle hermes footer (bare ❯)', () => {
    expect(detects(['❯'])).toBe(false)
  })

  it('does not fire on plain shell output', () => {
    expect(
      detects(['$ ls', 'Cargo.toml  src  target', '$ ']),
    ).toBe(false)
  })

  it('every signal is lowercase (the scan lowercases the row, not the table)', () => {
    for (const sig of WORKING_SIGNALS) {
      expect(sig).toBe(sig.toLowerCase())
    }
  })
})

describe('GROK_PERMISSION_TITLE_RE — grok gate title prefix', () => {
  it('matches the verbatim gate title', () => {
    expect(
      GROK_PERMISSION_TITLE_RE.test('⚠ Action Required - run rm -rf build'),
    ).toBe(true)
  })

  it('does not match grok working/idle titles', () => {
    expect(GROK_PERMISSION_TITLE_RE.test('⠙ - Thinking - grok')).toBe(false)
    expect(GROK_PERMISSION_TITLE_RE.test('grok')).toBe(false)
    expect(GROK_PERMISSION_TITLE_RE.test('my session - grok')).toBe(false)
  })

  it('does not match claude titles', () => {
    expect(GROK_PERMISSION_TITLE_RE.test('✳ Compacting')).toBe(false)
    expect(GROK_PERMISSION_TITLE_RE.test('⠋ my-project')).toBe(false)
  })

  it('anchors at the start — a transcript mention is not a gate', () => {
    expect(
      GROK_PERMISSION_TITLE_RE.test('note: ⚠ Action Required appeared'),
    ).toBe(false)
  })
})
