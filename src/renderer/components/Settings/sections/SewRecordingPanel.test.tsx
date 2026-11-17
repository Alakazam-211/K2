// @vitest-environment jsdom
// Settings → Chat Hosting → Recording (prd-home-call-recording-v1 §3, HR-11,
// HR-15): "Recording on this server" and "Record calls in DMs" are switches,
// both off until a person turns them on, on the policy route with the
// person confirm; "Record calls in DMs" waits for the server switch; the
// required rule says it also covers DMs.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: unknown): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  policies: [] as Array<Record<string, unknown>>,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: unknown) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { RecordingPanel, REQUIRED_ALSO_DMS, policyOn } from './SewRecordingPanel'

const row = (key: string, want: unknown = null, seen: unknown = null) => ({ key, want, seen, setBy: null, delivered: want === null || want === seen })

beforeEach(() => {
  h.get.mockReset()
  h.post.mockReset()
  h.policies = [row('recording.enabled'), row('recording.dm_enabled'), row('recording.required'), row('recording.audio_max_days', null, 90)]
  h.get.mockImplementation(async (_s, route) => {
    if (route === 'sew/recording/policies')
      return { ok: true, flags: { appRecording: true, recordingRequired: true }, policies: h.policies, locks: [], k5SpaceId: '9' }
    if (route === 'sew/home') return { ok: true, apps: [] }
    if (route === 'sew/app-spaces') return { ok: true, spaces: [] }
    throw new Error(`unexpected GET ${route}`)
  })
  h.post.mockImplementation(async () => ({ ok: true }))
})
afterEach(cleanup)

function toggleOf(key: string): HTMLInputElement | HTMLButtonElement {
  const el = document.querySelector(`[data-testid="sew-recording-switch"][data-key="${key}"]`)
  const t = el?.querySelector('input, button[role="switch"], [role="switch"]') as HTMLInputElement | HTMLButtonElement | null
  if (!t) throw new Error(`no toggle for ${key}`)
  return t
}
const isOn = (t: HTMLElement): boolean => (t as HTMLInputElement).checked === true || t.getAttribute('aria-checked') === 'true'

describe('the recording switches', () => {
  it('both start off; never there means off', async () => {
    render(<RecordingPanel sample={false} canMutate isOwner installId="ins_1" />)
    await screen.findByText('Recording on this server')
    expect(screen.getByText('Record calls in DMs')).toBeTruthy()
    expect(isOn(toggleOf('recording.enabled'))).toBe(false)
    expect(isOn(toggleOf('recording.dm_enabled'))).toBe(false)
    expect(policyOn(undefined)).toBe(false)
    expect(policyOn({ want: null, seen: null })).toBe(false)
    expect(policyOn({ want: null, seen: true })).toBe(true)
    expect(policyOn({ want: false, seen: true })).toBe(false)
  })

  it('turning one on goes to the policy route; Record calls in DMs waits for the server switch', async () => {
    render(<RecordingPanel sample={false} canMutate isOwner installId="ins_1" />)
    await screen.findByText('Recording on this server')
    expect((toggleOf('recording.dm_enabled') as HTMLInputElement).disabled).toBe(true)
    expect(screen.getByText('Turn on recording on this server first.')).toBeTruthy()
    fireEvent.click(toggleOf('recording.enabled'))
    await waitFor(() => expect(h.post).toHaveBeenCalledWith(expect.anything(), 'sew/recording/policy', { changes: { 'recording.enabled': true } }))
  })

  it('a person without the right sees them read-only', async () => {
    h.policies = [row('recording.enabled', true, true), row('recording.dm_enabled')]
    render(<RecordingPanel sample={false} canMutate={false} isOwner={false} installId="ins_1" />)
    await screen.findByText('Recording on this server')
    expect(isOn(toggleOf('recording.enabled'))).toBe(true)
    expect((toggleOf('recording.enabled') as HTMLInputElement).disabled).toBe(true)
    expect((toggleOf('recording.dm_enabled') as HTMLInputElement).disabled).toBe(true)
    expect(screen.getAllByText('Only the owner or an admin changes this.').length).toBe(2)
  })

  it('the required rule says it also records every DM call (HR-15)', async () => {
    render(<RecordingPanel sample={false} canMutate isOwner installId="ins_1" />)
    await screen.findByText('Record every call with two or more people')
    expect(screen.getByText(REQUIRED_ALSO_DMS)).toBeTruthy()
  })
})
