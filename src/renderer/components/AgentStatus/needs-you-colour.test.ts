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
    // The needs-you fill, inked in the dark on-warn (white read under 2:1).
    expect(s).toContain("badgeClass: 'bg-[var(--color-status-needs-you)] !text-[var(--color-on-warn)]'")
    expect(s).not.toContain('status-working')
  })

  it('tickets: waiting and the approval kind; needs discussion keeps its own orange', () => {
    // The status colours live in badges.tsx (the card and the header read
    // them from there).
    const badges = src('Feedback/badges.tsx')
    expect(/status === 'waiting'\s*\?\s*'[^']*--color-status-needs-you/.test(badges)).toBe(true)
    expect(src('Feedback/TicketCard.tsx')).toContain("from './badges'")
    for (const rel of ['Feedback/TicketCard.tsx', 'Feedback/badges.tsx']) {
      expect([rel, src(rel).includes('var(--color-status-working)')]).toEqual([rel, false])
    }
    expect(src('Feedback/badges.tsx')).toMatch(/kind === 'approval'\s*\?\s*'[^']*--color-status-needs-you/)
  })

  it("the desktop Thread's Zen kit maps --zen-needs-you to needs-you", () => {
    expect(src('Thread/ThreadZenKit.tsx')).toContain("'--zen-needs-you': 'var(--color-status-needs-you)'")
  })

  it('a 20+ day old chat is needs-you orange, not the working accent', () => {
    const s = src('ChatHistory/ChatHistory.tsx')
    // As text, the needs-you family's text tone (4.5:1 on every surface).
    expect(s).toContain("color: old ? 'var(--color-status-warn-text)'")
    expect(s).not.toContain('var(--color-status-working)')
  })

  it('the Usage page colours its series by harness, never with a status token', () => {
    // Usage v2 (0.46.0) replaced the Input / Output / Cache read series with
    // per-harness series colours, so no series can collide with working.
    // The series are the theme's chart tokens (0.46.11), slot by harness.
    const page = src('Settings/sections/usage/usage-page.ts')
    expect(page).toContain('var(--color-chart-1,')
    expect(page).toContain('var(--color-chart-other,')
    for (const rel of ['Settings/sections/TokenUsageSection.tsx', 'Settings/sections/usage/UsageChart.tsx', 'Settings/sections/usage/usage-page.ts']) {
      expect([rel, src(rel).includes('var(--color-status-working)')]).toEqual([rel, false])
    }
  })
})
