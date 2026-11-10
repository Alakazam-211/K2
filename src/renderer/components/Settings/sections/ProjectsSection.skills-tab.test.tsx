// @vitest-environment jsdom

// Settings → a workspace → Skills. The list comes through `invoke`, whose
// shape isn't guaranteed (a shim or an older build can answer null). A null
// answer is the empty state, never a crash; a failed read says so and offers
// Try again.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})), settingsReset: vi.fn() }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => 'owner') }))

import { ProjectsSection } from './ProjectsSection'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

type Answers = { skills: () => unknown; agents: () => unknown }

function answer({ skills, agents }: Answers): void {
  invoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'k2so_skills_list') return skills()
    if (cmd === 'k2so_agents_list') return agents()
    return null
  })
}

function openSkills(): void {
  useSettingsStore.setState({ initialProjectId: 'p1', initialWorkspaceTab: 'skills', projectsPage: null })
  render(<ProjectsSection />)
}

beforeEach(() => {
  invoke.mockReset()
  useProjectsStore.setState({ projects: [{ id: 'p1', name: 'ops', path: '/w/ops', color: '#3b82f6', workspaces: [], pinned: 0 }] } as never)
})

afterEach(() => {
  cleanup()
  useSettingsStore.setState({ initialProjectId: null, initialWorkspaceTab: null })
  useProjectsStore.setState({ projects: [] } as never)
})

describe('Skills tab', () => {
  it('shows the empty state when both lists answer null', async () => {
    answer({ skills: () => null, agents: () => null })
    openSkills()
    await screen.findByText(/No skills yet/)
    expect(screen.queryByText('Something went wrong')).toBeNull()
  })

  it('lists skills, and leaves the coordinator out of the rows', async () => {
    answer({
      skills: () => [
        { name: 'lead', title: null, lastModified: 1 },
        { name: 'backend-eng', title: 'Backend engineer', lastModified: 2 },
      ],
      agents: () => [{ name: 'lead', isCoordinator: true }],
    })
    openSkills()
    await screen.findByText('backend-eng')
    expect(screen.getByText('Backend engineer')).toBeTruthy()
    expect(screen.queryByText('lead')).toBeNull()
  })

  it('says so when the list can’t be read, and Try again reloads it', async () => {
    let fail = true
    answer({
      skills: () => {
        if (fail) throw new Error('nope')
        return [{ name: 'backend-eng', title: null, lastModified: 2 }]
      },
      agents: () => [],
    })
    openSkills()
    const box = await screen.findByTestId('skills-load-error')
    expect(box.textContent).toContain('Couldn’t load')
    expect(screen.queryByText(/No skills yet/)).toBeNull()
    // The button keeps its natural width beside the long hint: never wraps.
    const retry = screen.getByRole('button', { name: 'Try again' })
    expect(retry.className.split(' ')).toEqual(expect.arrayContaining(['flex-shrink-0', 'whitespace-nowrap']))
    fail = false
    fireEvent.click(retry)
    await waitFor(() => expect(screen.getByText('backend-eng')).toBeTruthy())
    expect(screen.queryByTestId('skills-load-error')).toBeNull()
  })

  it('the empty state says what a skill is and how to add one', async () => {
    answer({ skills: () => [], agents: () => [] })
    openSkills()
    const empty = await screen.findByTestId('skills-empty')
    expect(empty.textContent).toContain('No skills yet')
    expect(empty.textContent).toContain('Use New skill to write one.')
  })

  it('Remove asks first, as a destructive confirm, and a failed remove says so', async () => {
    const realConfirm = useConfirmDialogStore.getState().confirm
    const confirm = vi.fn(async () => true)
    useConfirmDialogStore.setState({ confirm } as never)
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    try {
      answer({ skills: () => [{ name: 'backend-eng', title: null, lastModified: 2 }], agents: () => [] })
      openSkills()
      fireEvent.click(await screen.findByRole('button', { name: 'Remove' }))
      await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
      expect((confirm.mock.calls[0] as unknown[])[0]).toMatchObject({ destructive: true, confirmLabel: 'Remove skill' })
      // No daemon in the test, so the write fails: the row says so.
      expect((await screen.findByText(/Couldn\u2019t remove \u201cbackend-eng\u201d/)).textContent).toContain('try again')
    } finally {
      useConfirmDialogStore.setState({ confirm: realConfirm } as never)
      spy.mockRestore()
    }
  })
})
