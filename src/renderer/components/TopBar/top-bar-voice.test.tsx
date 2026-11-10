// @vitest-environment jsdom
//
// Voice rooms §4.1: the headset sits inside the presence group (avatars,
// headset, then the divider). With the roster hidden it still shows, with
// its own divider; two dividers never touch.

import { afterEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/components/Timer/TimerButton', () => ({ default: () => <span data-stub="timer" /> }))
vi.mock('@/components/Timer/UsageButton', () => ({ default: () => <span data-stub="usage" /> }))
vi.mock('@/components/Timer/KeepAwakeButton', () => ({ default: () => <span data-stub="keep-awake" /> }))
vi.mock('@/components/Help/HelpButton', () => ({ default: () => <span data-stub="help" /> }))
vi.mock('@/components/SendFeedback/SendFeedbackButton', () => ({ default: () => <span data-stub="send-feedback" /> }))
vi.mock('./AdminProfileButton', () => ({ default: () => <span data-stub="admin-profile" /> }))
// The real button needs a voice server; here it always shows.
vi.mock('@/components/Voice/VoiceButton', () => ({
  default: ({ withPipe }: { withPipe?: boolean }) => (
    <>
      <span data-stub="voice" />
      {withPipe ? <span data-top-bar-pipe="" /> : null}
    </>
  ),
}))

import { cleanup, render } from '@testing-library/react'
import TopBarUtilities from './TopBarUtilities'
import { usePresenceStore, type RosterUser } from '@/stores/presence'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function user(name: string, role: RosterUser['role']): RosterUser {
  return { user: name, role, windowCount: 1, workspaces: [], connectedAt: 1 }
}

function clusterItems(): string[] {
  const cluster = document.querySelector('.no-drag.flex.items-center.gap-1')
  if (!cluster) throw new Error('no top-bar cluster')
  return Array.from(cluster.children).map((c) => {
    if (c.hasAttribute('data-top-bar-pipe')) return 'pipe'
    if (c.getAttribute('data-testid') === 'presence-roster') return 'presence'
    return c.getAttribute('data-stub') ?? `other:${c.tagName}`
  })
}

afterEach(() => {
  cleanup()
  usePresenceStore.setState({ roster: [], supported: true })
})

describe('the headset in the top bar', () => {
  it('with the roster showing: avatars, headset, divider, usage', () => {
    usePresenceStore.setState({ roster: [user('owner', 'owner'), user('appa', 'member')], supported: true })
    render(<TopBarUtilities />)
    expect(clusterItems().slice(0, 4)).toEqual(['presence', 'voice', 'pipe', 'usage'])
  })

  it('with the roster hidden: headset, its own divider, usage', () => {
    usePresenceStore.setState({ roster: [user('owner', 'owner')], supported: true })
    render(<TopBarUtilities />)
    const items = clusterItems()
    expect(items.slice(0, 3)).toEqual(['voice', 'pipe', 'usage'])
    expect(items.filter((it, i) => it === 'pipe' && items[i + 1] === 'pipe')).toEqual([])
  })
})
