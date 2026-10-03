// @vitest-environment jsdom
// research-spread-not-iterable-crash-v1: an older or odd server answered
// `chat/list` with an object (error envelope, wrapped shape). The drawer
// stored it and `[...filtered]` in the list memo crashed the window. The
// drawer must show its empty state instead. Fail loud — no skip.
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { screen, waitFor, cleanup } from '@testing-library/react'

const PROJECT = '/work/object-body'

const h = vi.hoisted(() => ({
  chatList: undefined as unknown,
  calls: [] as string[],
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly(vi.fn(async (route: string) => {
      h.calls.push(route)
      if (route === 'chat/list') return h.chatList
      if (route === 'chat/custom-names') return {}
      return []
    })),
    daemonCliPost: primaryOnly(vi.fn(async () => ({}))),
    RecoveringError: class RecoveringError extends Error {},
    isHostSwitchedError: () => false,
  }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
  emit: vi.fn(async () => {}),
}))

import ChatHistory from './ChatHistory'
import { useTabsStore } from '@/stores/tabs'
import { useHeartbeatSessionsStore } from '@/stores/heartbeat-sessions'

beforeEach(() => {
  h.calls.length = 0
  useTabsStore.setState({
    tabs: [],
    activeTabId: null,
    extraGroups: [],
    splitCount: 1,
    activeGroupIndex: 0,
  })
  useHeartbeatSessionsStore.setState({
    loadedFor: PROJECT,
    active: [],
    archived: [],
    loading: false,
    lastError: null,
  })
})

afterEach(() => {
  cleanup()
})

describe('Chats drawer with an object chat/list body', () => {
  for (const body of [{}, { sessions: [] }, { error: 'x' }, { rows: {} }]) {
    it(`renders the empty state for ${JSON.stringify(body)}`, async () => {
      h.chatList = body
      renderInPrimaryRoom(<ChatHistory projectPath={PROJECT} />)
      await waitFor(() => expect(h.calls).toContain('chat/list'))
      expect(await screen.findByText('No conversations yet')).toBeTruthy()
    })
  }
})
