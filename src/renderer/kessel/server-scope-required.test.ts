// Home M1 — every request helper refuses to compile without a ServerScope
// (MS1, MS52 p).
//
// The `@ts-expect-error` lines are the real test: `tsc -p tsconfig.web.json`
// fails if any of them stops being an error (an unused @ts-expect-error is
// itself an error), i.e. if someone gives a helper an optional scope or a
// silent default. At run time the same calls must also fail with the
// explicit scope guard's message (G9), never quietly hit the window's server.

import { describe, it, expect, vi } from 'vitest'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

import { daemonCliGet, daemonCliGetText, daemonCliPost } from '@/lib/daemon-cli'
import { terminalWrite } from '@/lib/terminal-daemon'
import { uploadToRemote } from '@/lib/upload-to-remote'
import { fetchProjectGroups } from '@/components/Projects/projects-api'
import { getDaemonWs } from './daemon-ws'
import { primaryScope } from './server-scope'

const SCOPE_REQUIRED = /a ServerScope is required as the first argument/

describe('request helpers require a scope', () => {
  it('daemonCliGet without a scope does not compile and is refused', async () => {
    const fetchSpy = vi.fn()
    vi.stubGlobal('fetch', fetchSpy)
    // @ts-expect-error — a route is not a ServerScope
    await expect(daemonCliGet('projects/list')).rejects.toThrow(SCOPE_REQUIRED)
    // @ts-expect-error — params cannot stand in for the scope either
    await expect(daemonCliGet('fs/read-dir', { path: '/x' })).rejects.toThrow(SCOPE_REQUIRED)
    expect(fetchSpy).not.toHaveBeenCalled()
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('daemonCliPost without a scope does not compile and is refused', async () => {
    // @ts-expect-error — a route is not a ServerScope
    await expect(daemonCliPost('workspace-layouts/save', {})).rejects.toThrow(SCOPE_REQUIRED)
  })

  it('daemonCliGetText without a scope does not compile and is refused', async () => {
    // @ts-expect-error — a route is not a ServerScope
    await expect(daemonCliGetText('timer/entries-export')).rejects.toThrow(SCOPE_REQUIRED)
  })

  it('getDaemonWs without a scope does not compile and is refused', () => {
    // @ts-expect-error — no argument
    expect(() => getDaemonWs()).toThrow(SCOPE_REQUIRED)
    // @ts-expect-error — undefined is not a ServerScope
    expect(() => getDaemonWs(undefined)).toThrow(SCOPE_REQUIRED)
    // @ts-expect-error — a look-alike object without the scope methods
    expect(() => getDaemonWs({ id: 'primary' })).toThrow(SCOPE_REQUIRED)
    expect(invokeMock).not.toHaveBeenCalled()
  })

  it('scope-taking wrappers do not compile without a scope', () => {
    // These are type-level checks only; the functions are never called.
    const typeOnly = (): void => {
      // @ts-expect-error — terminal wrappers take the scope first
      void terminalWrite('term-1', 'ls\n')
      // @ts-expect-error — uploads target one server
      void uploadToRemote('/tmp/a', '/srv/b')
      // @ts-expect-error — project-group API wrappers take the scope first
      void fetchProjectGroups()
    }
    expect(typeof typeOnly).toBe('function')
  })

  it('the primary scope is accepted (control)', () => {
    const fn: (s: ReturnType<typeof primaryScope>) => unknown = (s) => daemonCliGet(s, 'x')
    expect(typeof fn).toBe('function')
  })
})
