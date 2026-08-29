import { describe, it, expect, vi, beforeEach } from 'vitest'
import { composeAttachPlan } from './pick-compose-files'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}))

vi.mock('./is-web', () => ({
  isWebClient: () => false,
}))

const hostState = { activeHost: 'local' as string }
vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: { getState: () => hostState },
}))

vi.mock('@/stores/remote-folder-picker', () => ({
  useRemoteFolderPickerStore: { getState: () => ({ open: vi.fn() }) },
}))

beforeEach(() => {
  hostState.activeHost = 'local'
})

describe('composeAttachPlan', () => {
  it('local desktop uses the native OS picker (not hidden <input>)', () => {
    expect(composeAttachPlan({ activeHost: 'local', web: false })).toEqual({ kind: 'native' })
  })

  it('hosted web keeps the HTML file input', () => {
    expect(composeAttachPlan({ activeHost: 'local', web: true })).toEqual({ kind: 'web-input' })
  })

  it('remote host uses the in-app host picker', () => {
    expect(composeAttachPlan({ activeHost: 'k2e-01', web: false })).toEqual({ kind: 'remote' })
  })
})
