// @vitest-environment jsdom

// Settings → General → Server → Advanced: "New k5 command (Beta)". The card
// reads `GET /cli/k5-cli` on the focused server; a click posts exactly
// `{"on": <bool>}` to the same route and shows what the daemon answers.
// Anyone but the owner (the route refuses them), and a server from before
// the switch, sees no card.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

const h = vi.hoisted(() => ({
  answer: null as unknown,
  getError: null as Error | null,
  postError: null as Error | null,
  postAnswer: null as unknown,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: unknown }>,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

vi.mock('@/lib/daemon-cli', async (orig) => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    ...(await orig<typeof import('@/lib/daemon-cli')>()),
    daemonCliGet: primaryOnly(async (route: string) => {
      h.gets.push(route)
      if (h.getError) throw h.getError
      return h.answer
    }),
    daemonCliPost: primaryOnly(async (route: string, body: unknown) => {
      h.posts.push({ route, body })
      if (h.postError) throw h.postError
      return h.postAnswer
    }),
  }
})

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { primaryScope } from '@/kessel/server-scope'
import { K5_CLI_LABEL, K5CliBetaRow, k5CliStatusLine, readK5CliState } from './K5CliBetaRow'
import { GENERAL_MANIFEST } from './GeneralSection'
import { searchManifest } from '../searchManifest'

const sw = (): HTMLButtonElement => screen.getByRole('switch', { name: K5_CLI_LABEL }) as HTMLButtonElement

beforeEach(() => {
  h.answer = { on: false }
  h.getError = null
  h.postError = null
  h.postAnswer = null
  h.gets = []
  h.posts = []
})
afterEach(cleanup)

describe('New k5 command (Beta)', () => {
  it('shows the server value, off by default, read from k5-cli', async () => {
    render(<K5CliBetaRow scope={primaryScope()} />)
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('false'))
    expect(h.gets).toEqual(['k5-cli'])
    expect(screen.getByTestId('k5-cli-card').textContent).toContain('Advanced')
    expect(screen.getByTestId('k5-cli-card').textContent).toContain('Off by default')
    expect(screen.getByTestId('k5-cli-card').textContent).toContain('Not on Windows yet.')
  })

  it('posts {on} to k5-cli and shows what the daemon answers', async () => {
    h.postAnswer = { on: true, runs: 'binary', note: null, links: [] }
    render(<K5CliBetaRow scope={primaryScope()} />)
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('false'))
    fireEvent.click(sw())
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('true'))
    expect(h.posts).toEqual([{ route: 'k5-cli', body: { on: true } }])
    expect(screen.getByTestId('k5-cli-status').textContent).toContain('run the k5 program')

    h.postAnswer = { on: false, runs: 'script', note: null, links: [] }
    fireEvent.click(sw())
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('false'))
    expect(h.posts[1]).toEqual({ route: 'k5-cli', body: { on: false } })
    expect(screen.queryByTestId('k5-cli-status')).toBeNull()
  })

  it('on, but the links still run the script: says why', async () => {
    h.answer = { on: true, runs: 'script', note: 'this release has no k5 program' }
    render(<K5CliBetaRow scope={primaryScope()} />)
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('true'))
    expect(screen.getByTestId('k5-cli-status').textContent).toBe(
      'The k2 command still runs the shell script: this release has no k5 program.',
    )
  })

  it('a refused flip keeps the old value and says so', async () => {
    h.postError = new DaemonHttpError('save the setting: disk full', 500, null)
    render(<K5CliBetaRow scope={primaryScope()} />)
    await waitFor(() => expect(sw().getAttribute('aria-checked')).toBe('false'))
    fireEvent.click(sw())
    await waitFor(() => expect(screen.getByText('Couldn’t turn the new k5 command on.')).toBeTruthy())
    expect(sw().getAttribute('aria-checked')).toBe('false')
  })

  it('no card for anyone the route refuses, or a server from before it', async () => {
    for (const err of [new DaemonHttpError('Only the owner of this server turns the new k5 command on or off.', 403, 'owner_only'), new DaemonHttpError('route not found', 404, null)]) {
      h.getError = err
      const { container } = render(<K5CliBetaRow scope={primaryScope()} />)
      await waitFor(() => expect(h.gets.length).toBeGreaterThan(0))
      await Promise.resolve()
      expect(container.innerHTML).toBe('')
      expect(screen.queryByRole('switch', { name: K5_CLI_LABEL })).toBeNull()
      cleanup()
      h.gets = []
    }
  })

  it('reads the answer strictly', () => {
    expect(readK5CliState({ on: true, runs: 'binary' })).toEqual({ on: true, runs: 'binary', note: null })
    expect(() => readK5CliState({})).toThrow()
    expect(() => readK5CliState({ on: 'yes' })).toThrow()
    expect(() => readK5CliState(null)).toThrow()
    expect(k5CliStatusLine({ on: false })).toBeNull()
    expect(k5CliStatusLine({ on: false, runs: 'script', note: 'the setting is off' })).toBeNull()
  })

  it('is found by searching for k5, as a Beta setting on the Server tab', () => {
    const hits = searchManifest(GENERAL_MANIFEST, 'k5').map((e) => e.id)
    expect(hits).toContain('general.k5-cli-beta')
    const e = GENERAL_MANIFEST.find((x) => x.id === 'general.k5-cli-beta')
    expect(e?.beta).toBe(true)
    expect(e?.group).toBe('Server')
    expect(e?.description).toContain('Off by default.')
    expect(e?.description).toContain('Not on Windows yet.')
  })
})
