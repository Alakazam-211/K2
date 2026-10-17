// @vitest-environment jsdom
// Rosson 2026-10-16: the remote switch covers agents on other servers only.
// The per-agent and master switches carry the new copy, and the old
// "remote users" copy is gone. Fail loud — no skip.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  daemonCliPostQuery: vi.fn(async () => ({ success: true })),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPostQuery: h.daemonCliPostQuery,
}))

import { useSettingsStore } from '@/stores/settings'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import {
  AGENT_SWITCH_LABEL,
  AGENT_SWITCH_OFF_TEXT,
  AGENT_SWITCH_ON_TEXT,
  AGENT_SWITCH_OVERRIDE_TEXT,
  RemoteInstructToggle,
} from './RemoteInstructToggle'
import {
  AllowRemoteInstructRow,
  MASTER_SWITCH_LABEL,
  MASTER_SWITCH_OFF_TEXT,
  MASTER_SWITCH_ON_TEXT,
} from './AllowRemoteInstructRow'

function project(allow: 0 | 1): ProjectWithWorkspaces {
  return { id: 'p-1', name: 'sales', path: '/ws/sales', allowRemoteInstruct: allow } as unknown as ProjectWithWorkspaces
}

describe('agents-on-other-servers switch copy', () => {
  beforeEach(() => {
    cleanup()
    h.daemonCliPostQuery.mockClear()
    useSettingsStore.setState({ allowRemoteInstruct: false })
  })
  afterEach(() => cleanup())

  it('pins the exact per-agent and master copy', () => {
    expect(AGENT_SWITCH_LABEL).toBe('Let agents on other servers message this agent')
    expect(AGENT_SWITCH_OFF_TEXT).toBe(
      "Off: this agent isn't listed to connected servers and their agents can't message it. People who log in here can always message it.",
    )
    expect(MASTER_SWITCH_LABEL).toBe('Let agents on other servers message every agent')
  })

  it('per-agent switch off shows the label and the Off description', () => {
    render(<RemoteInstructToggle project={project(0)} fetchProjects={async () => {}} />)
    expect(screen.getByText(AGENT_SWITCH_LABEL)).toBeTruthy()
    expect(screen.getByText(AGENT_SWITCH_OFF_TEXT)).toBeTruthy()
    expect(screen.queryByText(AGENT_SWITCH_OVERRIDE_TEXT)).toBeNull()
    expect(screen.getByRole('switch', { name: AGENT_SWITCH_LABEL }).getAttribute('aria-checked')).toBe('false')
    expect(document.body.textContent).not.toMatch(/remote users/i)
  })

  it('per-agent switch on shows the On description', () => {
    render(<RemoteInstructToggle project={project(1)} fetchProjects={async () => {}} />)
    expect(screen.getByText(AGENT_SWITCH_ON_TEXT)).toBeTruthy()
    expect(screen.getByRole('switch', { name: AGENT_SWITCH_LABEL }).getAttribute('aria-checked')).toBe('true')
  })

  it('an off per-agent switch says the master overrides it', () => {
    useSettingsStore.setState({ allowRemoteInstruct: true })
    render(<RemoteInstructToggle project={project(0)} fetchProjects={async () => {}} />)
    expect(screen.getByText(AGENT_SWITCH_OVERRIDE_TEXT)).toBeTruthy()
  })

  it('flipping the per-agent switch writes the same internal route', async () => {
    const fetchProjects = vi.fn(async () => {})
    render(<RemoteInstructToggle project={project(0)} fetchProjects={fetchProjects} />)
    fireEvent.click(screen.getByRole('switch', { name: AGENT_SWITCH_LABEL }))
    await waitFor(() => expect(fetchProjects).toHaveBeenCalledTimes(1))
    expect(h.daemonCliPostQuery).toHaveBeenCalledTimes(1)
    const call = h.daemonCliPostQuery.mock.calls[0] as unknown[]
    expect(call[1]).toBe('remote-instruct')
    expect(call[2]).toEqual({ project: '/ws/sales', enable: '1' })
  })

  it('master row shows the new label and matching On/Off text', () => {
    const { unmount } = render(<AllowRemoteInstructRow />)
    expect(screen.getByText(MASTER_SWITCH_LABEL)).toBeTruthy()
    expect(screen.getByText(MASTER_SWITCH_OFF_TEXT)).toBeTruthy()
    expect(document.body.textContent).not.toMatch(/remote users/i)
    unmount()
    useSettingsStore.setState({ allowRemoteInstruct: true })
    render(<AllowRemoteInstructRow />)
    expect(screen.getByText(MASTER_SWITCH_ON_TEXT)).toBeTruthy()
  })
})
