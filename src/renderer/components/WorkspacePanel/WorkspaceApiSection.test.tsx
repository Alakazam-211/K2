// @vitest-environment jsdom
// Workspace drawer → API (Rosson 2026-11-15, "Is there a better icon for
// APIs?"): the header glyph is `ApiIcon` (Lucide `braces`, K2's stroke and
// caps), not a plus in a rounded square that reads as "add".
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, screen } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'api-keys/list') return { keys: [] }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { WorkspaceApiSection } from './WorkspaceApiSection'
import { renderInRoom, testRoom } from '@/test-utils/room'

afterEach(() => cleanup())

describe('Workspace drawer API section header', () => {
  it('uses ApiIcon (braces) in the accent colour at the header-glyph size', async () => {
    localStorage.clear()
    renderInRoom(
      testRoom({ tabs: createStore(() => ({})) }),
      <WorkspaceApiSection project={{ id: 'p1', name: 'cortana', path: '/ws/cortana' }} />,
    )
    const header = await screen.findByTitle('Collapse API')
    const icon = header.querySelector('svg[data-k2-icon="api"]')
    expect(icon).not.toBeNull()
    expect(icon?.getAttribute('stroke-linecap')).toBe('square')
    expect(icon?.getAttribute('stroke-width')).toBe('1.75')
    expect(icon?.getAttribute('width')).toBe('12')
    expect(icon?.getAttribute('class')).toContain('text-[var(--color-accent)]')
    // The old glyph: a plus inside a rounded square.
    expect(header.querySelector('rect[rx]')).toBeNull()
  })
})
