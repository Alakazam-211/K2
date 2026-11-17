// @vitest-environment jsdom
// The Thread's /model picker (0.46.13, K2 Design's spec): typed in the
// composer it opens pre-filtered, lists this harness's models under the
// server default, checks the current one, picks by keyboard, and the pick
// goes to the daemon's switch for this chat only. Its states have words.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ReactNode } from 'react'

const h = vi.hoisted(() => {
  const options = {
    sessionId: 'sess-1',
    harness: 'claude',
    switchable: true,
    current: 'haiku' as string | null,
    serverDefault: 'opus' as string | null,
    workspaceDefault: null,
  }
  const daemonCliGet = vi.fn(async (route: string): Promise<unknown> => {
    if (route === 'terminal/compose-history') return { items: [] }
    if (route === 'sessions/model/options') return { ...options }
    return []
  })
  const daemonCliPost = vi.fn(async (_route: string, _body?: unknown): Promise<unknown> => ({ ok: true }))
  return { daemonCliGet, daemonCliPost, options }
})

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(h.daemonCliGet),
    daemonCliPost: primaryOnly(h.daemonCliPost),
  }
})

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

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { ModelPicker, MODEL_PICKER_WORDS, modelPickerMove } from './ModelPicker'
import { modelPickerRows } from '@/lib/model-choices'

const CHROME: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'proj-1',
  displayName: 'sales',
  projectPath: '/ws/sales',
}

function Chrome({ children }: { children: ReactNode }) {
  return <SessionViewChromeContext.Provider value={CHROME}>{children}</SessionViewChromeContext.Provider>
}

function box(): HTMLTextAreaElement {
  const ta = document.querySelector('[data-compose-bar] textarea')
  if (!ta) throw new Error('no message box')
  return ta as HTMLTextAreaElement
}

function modelPosts(): unknown[][] {
  return (h.daemonCliPost.mock.calls as unknown[][]).filter((c) => c[0] === 'sessions/model')
}

function renderBar() {
  return renderInPrimaryRoom(
    <Chrome>
      <TerminalComposeBar sessionId="sess-1" draftId="proj-1" workspacePath="/ws/sales" sendDestination="thread" />
    </Chrome>,
  )
}

describe('the composer’s /model', () => {
  beforeEach(() => {
    cleanup()
    localStorage.clear()
    h.options.current = 'haiku'
    h.daemonCliPost.mockReset()
    h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
  })
  afterEach(() => cleanup())

  it('“/model op” opens the picker filtered to op; Enter switches this chat only', async () => {
    renderBar()
    await waitFor(() => expect(h.daemonCliGet).toHaveBeenCalledWith('sessions/model/options', { session_id: 'sess-1' }))
    fireEvent.change(box(), { target: { value: '/model op' } })
    const search = (await screen.findByTestId('compose-model-picker-search')) as HTMLInputElement
    expect(search.value).toBe('op')
    expect(box().value).toBe('')
    const options = screen.getAllByRole('option')
    // The server default is Opus too, so it stays in the filtered list.
    expect(options.map((o) => o.getAttribute('data-value'))).toEqual(['', 'opus'])
    expect(search.getAttribute('aria-activedescendant')).toBe(options[0].id)
    expect(screen.getByTestId('compose-model-picker-footer').textContent).toBe('This chat only, from your next message.')
    fireEvent.keyDown(search, { key: 'Enter' })
    await waitFor(() => expect(modelPosts()).toHaveLength(1))
    expect(modelPosts()[0][1]).toEqual({ session_id: 'sess-1', model: 'opus', thread: 'sales' })
    expect(screen.queryByTestId('compose-model-picker')).toBeNull()
  })

  it('lists the server default first, this harness’s models, and checks the current one', async () => {
    renderBar()
    const chip = await screen.findByTestId('compose-model')
    expect(chip.textContent).toBe('Haiku 5.5')
    fireEvent.click(chip)
    await screen.findByTestId('compose-model-picker')
    const options = screen.getAllByRole('option')
    expect(options[0].textContent).toContain('Server default (Opus 5.5)')
    expect(options.map((o) => o.getAttribute('data-value'))).toEqual(['', 'opus', 'sonnet', 'haiku'])
    const checked = options.filter((o) => o.getAttribute('aria-checked') === 'true')
    expect(checked.map((o) => o.getAttribute('data-value'))).toEqual(['haiku'])
    expect(checked[0].querySelector('[data-testid="compose-model-picker-check"]')).not.toBeNull()
  })

  it('the composer names the model on the server default too', async () => {
    h.options.current = 'opus'
    renderBar()
    expect((await screen.findByTestId('compose-model')).textContent).toBe('Opus 5.5')
  })

  it('a refused switch says why under the composer, and nothing changes', async () => {
    h.daemonCliPost.mockImplementation(async (route: string) => {
      if (route === 'sessions/model') throw new Error('The agent is mid-turn. Nothing was sent.')
      return { ok: true }
    })
    renderBar()
    const chip = await screen.findByTestId('compose-model')
    fireEvent.click(chip)
    const search = await screen.findByTestId('compose-model-picker-search')
    fireEvent.change(search, { target: { value: 'sonnet' } })
    fireEvent.keyDown(search, { key: 'Enter' })
    const err = await screen.findByTestId('compose-model-error')
    expect(err.textContent).toContain('mid-turn')
    expect(screen.getByTestId('compose-model').textContent).toBe('Haiku 5.5')
  })
})

describe('the picker on its own', () => {
  afterEach(() => cleanup())
  const rows = (q: string) => modelPickerRows({ harness: 'claude', mode: 'thread', serverDefault: 'opus', query: q })

  it('says loading, no match, and can’t load with Retry', () => {
    const onRetry = vi.fn()
    const base = { anchor: null, rows, checked: '', footer: 'f', label: 'Model', onPick: () => {}, onClose: () => {} }
    const r = render(<ModelPicker {...base} state="loading" />)
    expect(screen.getByRole('status').textContent).toBe('Loading models…')
    r.rerender(<ModelPicker {...base} state="error" onRetry={onRetry} />)
    expect(screen.getByRole('alert').textContent).toContain('Couldn’t load models')
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(onRetry).toHaveBeenCalled()
    r.unmount()
    render(<ModelPicker {...base} state="ready" initialQuery="xyz" />)
    expect(screen.getByTestId('model-picker-no-match').textContent).toBe('No model matches ‘xyz’.')
  })

  it('↑↓ move, Esc closes and gives focus back', () => {
    expect(modelPickerMove(0, 'ArrowDown', 4)).toBe(1)
    expect(modelPickerMove(3, 'ArrowDown', 4)).toBe(3)
    expect(modelPickerMove(0, 'ArrowUp', 4)).toBe(0)
    expect(modelPickerMove(0, 'x', 4)).toBeNull()
    const onClose = vi.fn()
    render(<ModelPicker anchor={null} rows={rows} checked="" state="ready" footer="f" label="Model" onPick={() => {}} onClose={onClose} />)
    const search = screen.getByTestId('model-picker-search')
    fireEvent.keyDown(search, { key: 'ArrowDown' })
    expect(search.getAttribute('aria-activedescendant')).toBe(screen.getAllByRole('option')[1].id)
    fireEvent.keyDown(search, { key: 'Escape' })
    expect(onClose).toHaveBeenCalledWith(true)
  })
})
