// plan-agent-status-unify-v1 colour rule (Rosson 2026-10-16): working is the
// theme's accent (`--color-status-working`); orange (`--color-status-needs-
// you`) means the person is needed. After part 1 moved working to the
// accent, every use of the working token was audited (part 2): "needs the
// person" moved to needs-you, "the agent is busy" kept the accent. This
// guards the moved ones.

import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const components = join(dirname(fileURLToPath(import.meta.url)), '..')
const src = (rel: string): string => readFileSync(join(components, rel), 'utf8')

describe('needs the person = needs-you, never the working token', () => {
  it('the Tickets page-tab badge (tickets waiting on you)', () => {
    const s = src('TopBar/PageTabs.tsx')
    expect(s).toContain("badgeClass: 'bg-[var(--color-status-needs-you)]'")
    expect(s).not.toContain('status-working')
  })

  it('tickets: waiting and the approval kind; needs discussion keeps its own orange', () => {
    for (const rel of ['Feedback/TicketCard.tsx', 'Feedback/badges.tsx']) {
      const s = src(rel)
      expect([rel, /status === 'waiting'\s*\?\s*'[^']*--color-status-needs-you/.test(s)]).toEqual([rel, true])
      expect([rel, s.includes('var(--color-status-working)')]).toEqual([rel, false])
    }
    expect(src('Feedback/badges.tsx')).toMatch(/kind === 'approval'\s*\?\s*'[^']*--color-status-needs-you/)
  })

  it("the desktop Thread's Zen kit maps --zen-needs-you to needs-you", () => {
    expect(src('Thread/ThreadZenKit.tsx')).toContain("'--zen-needs-you': 'var(--color-status-needs-you)'")
  })

  it('a 20+ day old chat is needs-you orange, not the working accent', () => {
    const s = src('ChatHistory/ChatHistory.tsx')
    expect(s).toContain("color: old ? 'var(--color-status-needs-you)'")
    expect(s).not.toContain('var(--color-status-working)')
  })

  it("token usage's cache-read series is not the accent (Input already is)", () => {
    const s = src('Settings/sections/TokenUsageSection.tsx')
    expect(s).toContain("input: 'var(--color-accent)'")
    expect(s).not.toContain("cacheRead: 'var(--color-status-working)'")
  })
})
