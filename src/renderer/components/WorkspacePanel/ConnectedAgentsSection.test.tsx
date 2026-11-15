// @vitest-environment jsdom
// Workspace drawer → Connected Agents (Rosson 2026-11-15): each connected
// agent paints as its avatar (the shared `ProjectAvatar`: octagon in
// Terminal, round in Glass), never an 8px colour square, and the section
// header uses the app's agent icon (`AgentsIcon`, Lucide `bot` squared).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, screen, waitFor } from '@testing-library/react'
import { createStore } from 'zustand/vanilla'

const getCalls: Array<{ route: string; params: unknown }> = []

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => 0) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params: unknown) => {
    getCalls.push({ route, params })
    if (route === 'connections') {
      return {
        connections: [
          { projectId: 'p2', projectName: 'halsey', path: '/ws/halsey', reachable: true },
          { remote: true, agent: 'ai', host: 'rpm.k2.dev', address: 'ai::rpm.k2.dev' },
          { remote: true, agent: 'old', host: 'gone.k2.dev', address: 'old::gone.k2.dev', remoteWorkspaceGone: true },
        ],
      }
    }
    if (route === 'projects/get-icon') return { found: false, dataUrl: null }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { ConnectedAgentsSection } from './ConnectedAgentsSection'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { useHomeAvatarStore } from '@/lib/home-avatars'

const PIC = 'data:image/png;base64,iVBORw0KGgo='

function project(id: string, name: string, color: string): ProjectWithWorkspaces {
  return { id, name, path: `/ws/${name}`, color, iconUrl: null, workspaces: [] } as unknown as ProjectWithWorkspaces
}

function render(): void {
  localStorage.clear()
  const room = testRoom({
    tabs: createStore(() => ({})),
    projects: [project('p1', 'cortana', '#3b82f6'), project('p2', 'halsey', '#22c55e')],
    activeProjectId: 'p1',
  })
  renderInRoom(room, <ConnectedAgentsSection projectId="p1" />)
}

/** The row element (button or div) that holds `title`. */
function rowOf(title: string): HTMLElement {
  const row = screen.getByText(title).closest('button, div')
  if (!row) throw new Error(`no row for ${title}`)
  return row as HTMLElement
}

function avatarOf(title: string): HTMLElement {
  const el = rowOf(title).querySelector<HTMLElement>('[data-k2-avatar]')
  if (!el) throw new Error(`row ${title} has no avatar`)
  return el
}

afterEach(() => {
  cleanup()
  getCalls.length = 0
  useHomeAvatarStore.setState({ cached: {}, local: {}, colors: {} })
})

describe('Connected Agents rows paint avatars', () => {
  it('a local row paints the workspace avatar: its initial on its colour', async () => {
    render()
    await screen.findByText('halsey')
    const avatar = avatarOf('halsey')
    expect(avatar.textContent).toBe('H')
    await waitFor(() => expect(avatar.style.backgroundColor).toBe('rgb(34, 197, 94)'))
    expect(avatar.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(avatar.style.width).toBe('18px')
    expect(rowOf('halsey').querySelector('[data-conn-avatar="local"]')).not.toBeNull()
  })

  it('a remote row paints the Home picture for its address, bordered in its own colour', async () => {
    useHomeAvatarStore.setState({
      cached: { 'ai::rpm.k2.dev': { dataUrl: PIC, missing: false, fetchedAt: Date.now(), sha256: null } },
      colors: { 'ai::rpm.k2.dev': '#f97316' },
    })
    render()
    await screen.findByText('ai')
    const avatar = avatarOf('ai')
    expect(avatar.querySelector('img')?.getAttribute('src')).toBe(PIC)
    expect(avatar.style.border).toContain('rgb(249, 115, 22)')
    expect(rowOf('ai').querySelector('[data-conn-avatar="remote"]')).not.toBeNull()
  })

  it('a remote row with no cached picture paints its initial on the accent colour', async () => {
    render()
    await screen.findByText('ai')
    const avatar = avatarOf('ai')
    expect(avatar.textContent).toBe('A')
    expect(avatar.style.backgroundColor).toBe('var(--color-accent)')
    // A remote agent's path means nothing here: no icon lookup for it (the
    // local row's lookup may already sit in ProjectAvatar's shared cache).
    const looked = getCalls.filter((c) => c.route === 'projects/get-icon').map((c) => (c.params as { path: string }).path)
    expect(looked.filter((p) => p !== '/ws/halsey')).toEqual([])
  })

  it('a gone row paints a dimmed neutral avatar and keeps the struck-through name', async () => {
    render()
    await screen.findByText('old')
    const avatar = avatarOf('old')
    expect(avatar.textContent).toBe('O')
    expect(avatar.style.backgroundColor).toBe('var(--color-neutral)')
    const wrap = rowOf('old').querySelector<HTMLElement>('[data-conn-avatar="remote"]')
    expect(wrap?.className).toContain('opacity-60')
    expect(wrap?.className).toContain('grayscale')
    expect(screen.getByText('old').className).toContain('line-through')
  })

  it('no row paints a colour square swatch', async () => {
    render()
    await screen.findByText('old')
    for (const title of ['halsey', 'ai', 'old']) {
      const squares = Array.from(rowOf(title).querySelectorAll<HTMLElement>('span.w-2.h-2'))
      expect(squares).toEqual([])
    }
  })
})

describe('Connected Agents header', () => {
  it('uses the app agent icon (AgentsIcon), not the old rounded robot', async () => {
    render()
    await screen.findByText('halsey')
    const header = screen.getByTitle('Collapse Connected Agents')
    const icon = header.querySelector('svg[data-k2-icon="agents"]')
    expect(icon).not.toBeNull()
    expect(icon?.getAttribute('stroke-linecap')).toBe('square')
    expect(icon?.getAttribute('class')).toContain('text-[var(--color-accent)]')
    expect(icon?.getAttribute('width')).toBe('12')
    expect(header.querySelector('rect[rx]')).toBeNull()
  })
})
