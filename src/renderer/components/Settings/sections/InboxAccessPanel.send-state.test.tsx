// @vitest-environment jsdom
// Appa/Baden (macOS 0.45.4): Settings showed appa's level "Send" on the
// linked Gmail while every send was gated by appa's workspace agent-send
// gate. The panel must show the EFFECTIVE state next to a Send level
// (daemon `sendState`) and offer "Turn on sending" when it is blocked.
import React, { useState } from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const calls = vi.hoisted(() => ({
  agentSend: [] as Array<{ workspace: string; agentSend: string }>,
  failAgentSend: false,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/stores/projects', () => ({
  useProjectsStore: (sel: (s: { projects: Array<{ id: string; name: string }> }) => unknown) =>
    sel({ projects: [{ id: 'p-baden', name: 'baden' }, { id: 'p-appa', name: 'appa' }] }),
}))
vi.mock('./email-api', async (orig) => {
  const real = await orig<typeof import('./email-api')>()
  return {
    ...real,
    setWorkspaceAgentSend: vi.fn(async (b: { workspace: string; agentSend: string }) => {
      calls.agentSend.push(b)
      if (calls.failAgentSend) throw new Error('role_required')
      return { ok: true }
    }),
    setInboxLevel: vi.fn(async () => ({ ok: true })),
  }
})

import { InboxAccessPanel, sendStateNote } from './InboxAccessPanel'
import type { Inbox, InboxParticipant } from './email-api'

function inbox(appa: Partial<InboxParticipant>): Inbox {
  return {
    address: 'baden@gmail.com',
    source: 'linked',
    displayName: null,
    status: 'connected',
    primary: {
      projectId: 'p-baden', workspace: 'baden', level: 'send', canManage: true, canDelete: false,
      agentSend: 'on', sendState: 'on',
    },
    grants: [{
      projectId: 'p-appa', workspace: 'appa', level: 'send', canManage: false, canDelete: false,
      agentSend: 'off', sendState: 'blocked_off', ...appa,
    }],
    yourLevel: null,
    maxLevel: 'send',
    host: 'imap.gmail.com',
  }
}

function Harness({ start, canMutate = true }: { start: Inbox; canMutate?: boolean }): React.JSX.Element {
  const [row, setRow] = useState(start)
  return (
    <InboxAccessPanel
      inbox={row}
      canMutate={canMutate}
      patchInbox={(_a, patch) => setRow((r) => patch(r))}
    />
  )
}

beforeEach(() => {
  calls.agentSend = []
  calls.failAgentSend = false
})
afterEach(() => cleanup())

describe('sendStateNote', () => {
  it('words every daemon state; blocked ones say why', () => {
    expect(sendStateNote('blocked_off')).toEqual({
      blocked: true,
      text: expect.stringContaining("agent sending is off"),
    })
    expect(sendStateNote('blocked_approval')?.blocked).toBe(true)
    expect(sendStateNote('blocked_approval')?.text).toContain('A linked inbox needs On')
    expect(sendStateNote('approval')).toEqual({ blocked: false, text: expect.stringContaining('approval') })
    expect(sendStateNote('on')?.blocked).toBe(false)
    expect(sendStateNote('no_send_level')).toBeNull()
    expect(sendStateNote(undefined)).toBeNull()
  })
})

describe('InboxAccessPanel effective send state', () => {
  it("Baden's case: appa at Send with the gate off shows BLOCKED and a Turn on button", async () => {
    render(<Harness start={inbox({})} />)
    const line = screen.getByTestId('send-state-p-appa')
    expect(line.getAttribute('data-send-state')).toBe('blocked_off')
    expect(line.textContent).toContain("Send is blocked: this workspace's agent sending is off")
    // The primary (gate on) says it works, with no button.
    const primary = screen.getByTestId('send-state-p-baden')
    expect(primary.textContent).toContain('Sending is on')
    expect(primary.querySelector('button')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Turn on sending' }))
    await waitFor(() =>
      expect(screen.getByTestId('send-state-p-appa').getAttribute('data-send-state')).toBe('on'),
    )
    expect(calls.agentSend).toEqual([{ workspace: 'p-appa', agentSend: 'on' }])
    expect(screen.queryByRole('button', { name: 'Turn on sending' })).toBeNull()
  })

  it('a refused turn-on rolls back and shows the error', async () => {
    calls.failAgentSend = true
    render(<Harness start={inbox({})} />)
    fireEvent.click(screen.getByRole('button', { name: 'Turn on sending' }))
    await waitFor(() => expect(screen.getByText(/role_required/)).toBeTruthy())
    expect(screen.getByTestId('send-state-p-appa').getAttribute('data-send-state')).toBe('blocked_off')
  })

  it('linked + approval is blocked (no queue for linked)', () => {
    render(<Harness start={inbox({ agentSend: 'approval', sendState: 'blocked_approval' })} />)
    expect(screen.getByTestId('send-state-p-appa').textContent).toContain('A linked inbox needs On')
    expect(screen.getByRole('button', { name: 'Turn on sending' })).toBeTruthy()
  })

  it('a read-only viewer sees the blocked state but cannot flip it', () => {
    render(<Harness start={inbox({})} canMutate={false} />)
    const btn = screen.getByRole('button', { name: 'Turn on sending' }) as HTMLButtonElement
    expect(btn.disabled).toBe(true)
  })

  it('no line below Send for a Draft level or an older daemon without sendState', () => {
    render(<Harness start={inbox({ level: 'draft', sendState: 'no_send_level' })} />)
    expect(screen.queryByTestId('send-state-p-appa')).toBeNull()
    cleanup()
    render(<Harness start={inbox({ agentSend: undefined, sendState: undefined })} />)
    expect(screen.queryByTestId('send-state-p-appa')).toBeNull()
  })

  it('copy no longer claims sending is immediate', () => {
    render(<Harness start={inbox({})} />)
    expect(document.body.textContent).not.toContain('Sending is immediate for now')
    expect(document.body.textContent).toContain("agent sending is on")
  })
})
