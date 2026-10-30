// @vitest-environment jsdom
// S4: files a person picks in a Thread's message box ride the post as
// `attachments` (thread-attachments-v1), not as path text. Fail loud.
//
//   1. With the feature: picked files are chips (images with a picture);
//      a file in the workspace attaches where it is, one outside is
//      uploaded into `.k2/downloads` first; Send posts `attachments` and
//      no path text (files alone are a message); the chips clear.
//   2. Without the feature: today's path text in the draft.
//   3. Another server: the file is uploaded to that server and attached.
//   4. A drop the window routed (`k2so:compose-insert` with paths) becomes
//      chips too.
//   5. A refused file keeps the draft and the chips and shows why.
//   6. The PTY bar is untouched.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    return []
  }),
  daemonCliPost: vi.fn(),
  picked: [] as string[][],
  executeRemoteDrop: vi.fn(),
  executeBrowserFileDrop: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))
vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: vi.fn(async () => h.picked.shift() ?? null),
  pickRemoteComposeFile: vi.fn(async () => null),
}))
vi.mock('@/lib/handle-remote-drop', () => ({
  executeRemoteDrop: h.executeRemoteDrop,
  executeBrowserFileDrop: h.executeBrowserFileDrop,
}))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))

import { TerminalComposeBar } from './TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { useContextMenuStore } from '@/stores/context-menu'

const WS = '/Users/me/ai/sales'

function scopeOf(hostKey: string, remote: boolean, features: string[]): ServerScope {
  return { ...fakeScope(hostKey, { remote }), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function renderBar(opts: { scope: ServerScope; dest?: 'thread' | 'pty'; isPrimary?: boolean; ws?: string }): void {
  const ws = opts.ws ?? WS
  const room = testRoom({
    tabs: {},
    scope: opts.scope,
    isPrimary: opts.isPrimary ?? true,
    projects: [{ id: 'p', name: 'sales', path: ws } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <TerminalComposeBar sessionId="sess-1" workspacePath={ws} sendDestination={opts.dest ?? 'thread'} />
    </SessionViewChromeContext.Provider>,
  )
}

function uploadsTo(dir: string) {
  return async (_s: ServerScope, local: string[], _t: unknown, _c: unknown, build?: (r: string[]) => string) => {
    const remote = local.map((p) => `${dir}/${p.split('/').pop()}`)
    return build ? build(remote) : null
  }
}

function threadPosts(): unknown[] {
  return h.daemonCliPost.mock.calls.filter((c) => c[1] === 'thread/post').map((c) => c[2])
}

async function pick(paths: string[]): Promise<void> {
  h.picked.push(paths)
  // "+" opens its menu (attach, then Save to stash); pick attach.
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Attach or save' }))
  })
  await act(async () => {
    useContextMenuStore.getState().selectItem('attach')
  })
}

function chipNames(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-attachment]')).map((el) => el.getAttribute('data-zen-attachment') ?? '')
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  h.daemonCliPost.mockReset()
  h.daemonCliGet.mockClear()
  h.executeRemoteDrop.mockReset()
  h.executeBrowserFileDrop.mockReset()
  h.picked.length = 0
})

afterEach(() => {
  cleanup()
})

const ON = ['thread-attachments-v1']

describe('S4: Thread compose files become attachments', () => {
  it('picked files are chips; Send posts them as attachments, files alone are a message', async () => {
    h.executeRemoteDrop.mockImplementation(uploadsTo(`${WS}/.k2/downloads`))
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9, kind: 'text', from: 'owner', body: '' })
    renderBar({ scope: scopeOf('local', false, ON) })
    await pick([`${WS}/out/hero.png`, '/Users/me/Desktop/notes.pdf'])
    await waitFor(() => expect(chipNames()).toEqual(['hero.png', 'notes.pdf']))
    // The image chip has its picture; the draft has no path text.
    expect(document.querySelector('[data-compose-chip-thumb]')).not.toBeNull()
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    expect(box.value).toBe('')
    // Only the outside file was uploaded.
    expect(h.executeRemoteDrop.mock.calls[0][1]).toEqual(['/Users/me/Desktop/notes.pdf'])

    const send = screen.getByRole('button', { name: 'Send message' })
    expect(send.hasAttribute('disabled')).toBe(false)
    await act(async () => {
      fireEvent.click(send)
    })
    expect(threadPosts()).toEqual([
      {
        addr: 'sales',
        text: '',
        via: 'compose',
        attachments: [{ path: `${WS}/out/hero.png` }, { path: `${WS}/.k2/downloads/notes.pdf` }],
      },
    ])
    await waitFor(() => expect(chipNames()).toEqual([]))
  })

  it('text and files ride together; the text has no paths in it', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9 })
    renderBar({ scope: scopeOf('local', false, ON) })
    await pick([`${WS}/docs/plan.md`])
    await waitFor(() => expect(chipNames()).toEqual(['plan.md']))
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'Have a look' } })
    await act(async () => {
      fireEvent.keyDown(box, { key: 'Enter' })
    })
    expect(threadPosts()).toEqual([
      { addr: 'sales', text: 'Have a look', via: 'compose', attachments: [{ path: `${WS}/docs/plan.md` }] },
    ])
  })

  it('without the feature: today\'s path text in the draft, no attachments', async () => {
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9 })
    renderBar({ scope: scopeOf('local', false, []) })
    await pick([`${WS}/docs/plan.md`])
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    await waitFor(() => expect(box.value).toContain(`${WS}/docs/plan.md`))
    expect(chipNames()).toEqual([])
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    })
    const body = threadPosts()[0] as Record<string, unknown>
    expect(body.text).toBe(`${WS}/docs/plan.md`)
    expect('attachments' in body).toBe(false)
  })

  it('another server: the local file is uploaded there, and that path attached', async () => {
    const remoteWs = '/home/k2/ai/sales'
    h.executeRemoteDrop.mockImplementation(uploadsTo(`${remoteWs}/.k2/downloads`))
    h.daemonCliPost.mockResolvedValue({ ok: true, id: 'd1', seq: 9 })
    renderBar({ scope: scopeOf('box.example', true, ON), isPrimary: false, ws: remoteWs })
    await pick(['/Users/me/Desktop/shot.png'])
    await waitFor(() => expect(chipNames()).toEqual(['shot.png']))
    expect(h.executeRemoteDrop.mock.calls[0][3]).toEqual({ workspacePath: remoteWs })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    })
    expect((threadPosts()[0] as { attachments: unknown }).attachments).toEqual([
      { path: `${remoteWs}/.k2/downloads/shot.png` },
    ])
  })

  it('a drop the window routed (uploaded already) becomes chips', async () => {
    renderBar({ scope: scopeOf('box.example', true, ON), isPrimary: false })
    const bar = document.querySelector('[data-compose-bar]') as HTMLElement
    await act(async () => {
      bar.dispatchEvent(
        new CustomEvent('k2so:compose-insert', {
          detail: { data: `'${WS}/.k2/downloads/a.png' `, paths: [`${WS}/.k2/downloads/a.png`], uploaded: true },
        }),
      )
    })
    await waitFor(() => expect(chipNames()).toEqual(['a.png']))
    expect((screen.getByRole('textbox') as HTMLTextAreaElement).value).toBe('')
    expect(h.executeRemoteDrop).not.toHaveBeenCalled()
  })

  it('a refused file: the draft and the chips stay, with the daemon\'s reason', async () => {
    h.daemonCliPost.mockRejectedValue(
      new DaemonHttpError('Secrets travel through the vault (`k2 vault`), not the Thread', 400, 'attachment_secret'),
    )
    renderBar({ scope: scopeOf('local', false, ON) })
    await pick([`${WS}/.env.local`])
    await waitFor(() => expect(chipNames()).toEqual(['.env.local']))
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    fireEvent.change(box, { target: { value: 'the config' } })
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Send message' }))
    })
    expect(threadPosts()).toHaveLength(1)
    await waitFor(() => expect(box.value).toBe('the config'))
    expect(chipNames()).toEqual(['.env.local'])
    expect(screen.getByTestId('compose-thread-addr-error').textContent).toBe(
      'Secrets travel through the vault (`k2 vault`), not the Thread',
    )
  })

  it('× on a chip takes the file off', async () => {
    renderBar({ scope: scopeOf('local', false, ON) })
    await pick([`${WS}/a.md`, `${WS}/b.md`])
    await waitFor(() => expect(chipNames()).toEqual(['a.md', 'b.md']))
    fireEvent.click(screen.getByRole('button', { name: 'Remove a.md' }))
    expect(chipNames()).toEqual(['b.md'])
  })

  it('the PTY bar is untouched: path text, no attachments', async () => {
    renderBar({ scope: scopeOf('local', false, ON), dest: 'pty' })
    await pick([`${WS}/docs/plan.md`])
    const box = screen.getByRole('textbox') as HTMLTextAreaElement
    await waitFor(() => expect(box.value).toContain(`${WS}/docs/plan.md`))
    expect(chipNames()).toEqual([])
  })
})
