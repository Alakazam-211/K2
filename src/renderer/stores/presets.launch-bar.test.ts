// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as daemonCli from '@/lib/daemon-cli'
import { activeHostKey, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import {
  readShowLaunchBar,
  showLaunchBarStorageKey,
  usePresetsStore,
  type AgentPreset,
} from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const REMOTE: ConnectHost = {
  id: 'box',
  label: 'Box',
  hostname: 'box.example',
  port: 60710,
  secure: true,
  token: 'tok',
  remember: false,
  lastConnectedAt: null,
}

function preset(over: Partial<AgentPreset> & Pick<AgentPreset, 'id' | 'label'>): AgentPreset {
  return {
    command: 'claude',
    icon: null,
    enabled: 1,
    sortOrder: 0,
    isBuiltIn: 1,
    createdAt: 1,
    ...over,
  }
}

describe('show launch bar persistence', () => {
  beforeEach(() => {
    localStorage.clear()
    useConnectHostStore.setState({ activeHost: 'local' })
    usePresetsStore.setState({ presets: [], showPresetsBar: true })
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
    })
  })

  afterEach(() => {
    useConnectHostStore.setState({ activeHost: 'local' })
    localStorage.clear()
  })

  it('writes one host key and a fresh read of that key stays off', () => {
    usePresetsStore.getState().setShowLaunchBar(false)
    expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')
    expect(readShowLaunchBar('local')).toBe(false)
    usePresetsStore.setState({ showPresetsBar: true })
    usePresetsStore.setState({ showPresetsBar: readShowLaunchBar('local') })
    expect(usePresetsStore.getState().showPresetsBar).toBe(false)
  })

  it('a missing key means shown, including after a host switch', () => {
    usePresetsStore.getState().setShowLaunchBar(false)
    useConnectHostStore.getState().selectHost(REMOTE)
    const remoteKey = activeHostKey(REMOTE)
    expect(localStorage.getItem(showLaunchBarStorageKey(remoteKey))).toBeNull()
    expect(readShowLaunchBar(remoteKey)).toBe(true)
    expect(usePresetsStore.getState().showPresetsBar).toBe(true)

    usePresetsStore.getState().setShowLaunchBar(false)
    expect(localStorage.getItem(showLaunchBarStorageKey(remoteKey))).toBe('0')
    useConnectHostStore.getState().selectHost('local')
    expect(usePresetsStore.getState().showPresetsBar).toBe(false)
    expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')
  })

  it('fetchPresets does not write or reset the flag', async () => {
    usePresetsStore.getState().setShowLaunchBar(false)
    const setItem = vi.spyOn(Storage.prototype, 'setItem')
    const get = vi.spyOn(daemonCli, 'daemonCliGet').mockResolvedValue([])
    try {
      await usePresetsStore.getState().fetchPresets()
      expect(usePresetsStore.getState().showPresetsBar).toBe(false)
      expect(localStorage.getItem(showLaunchBarStorageKey('local'))).toBe('0')
      expect(get).toHaveBeenCalledWith('presets/list')
      expect(setItem).not.toHaveBeenCalled()
    } finally {
      setItem.mockRestore()
      get.mockRestore()
    }
  })

  it('launchPreset tab mode uses the passed group, not activeGroupIndex', async () => {
    useTabsStore.setState({
      splitCount: 2,
      extraGroups: [{ tabs: [], activeTabId: null }],
      activeGroupIndex: 0,
    })
    usePresetsStore.setState({
      presets: [preset({ id: 'c', label: 'Claude', command: 'claude' })],
    })
    const post = vi.spyOn(daemonCli, 'daemonCliPost').mockResolvedValue({ ok: true, installed: false })
    try {
      await usePresetsStore.getState().launchPreset('c', '/tmp/proj', 'tab', 1)
      expect(post).toHaveBeenCalledWith('agents/ensure-cli', { program: 'claude' })
      expect(useTabsStore.getState().tabs).toHaveLength(0)
      expect(useTabsStore.getState().activeGroupIndex).toBe(0)
      expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.title)).toEqual(['Claude'])
    } finally {
      post.mockRestore()
    }
  })

  it('launchPreset without a group still follows activeGroupIndex', async () => {
    useTabsStore.setState({
      splitCount: 2,
      extraGroups: [{ tabs: [], activeTabId: null }],
      activeGroupIndex: 1,
    })
    usePresetsStore.setState({
      presets: [preset({ id: 'c', label: 'Claude', command: 'claude' })],
    })
    const post = vi.spyOn(daemonCli, 'daemonCliPost').mockResolvedValue({ ok: true, installed: false })
    try {
      await usePresetsStore.getState().launchPreset('c', '/tmp/proj', 'tab')
      expect(useTabsStore.getState().tabs).toHaveLength(0)
      expect(useTabsStore.getState().extraGroups[0].tabs.map((t) => t.title)).toEqual(['Claude'])
    } finally {
      post.mockRestore()
    }
  })

  it('a new store load with this host key stays off, and another host with no key stays on', async () => {
    localStorage.setItem(showLaunchBarStorageKey('local'), '0')
    vi.resetModules()
    const localMod = await import('@/stores/presets')
    expect(localMod.usePresetsStore.getState().showPresetsBar).toBe(false)

    vi.resetModules()
    const hostMod = await import('@/stores/connect-host')
    hostMod.useConnectHostStore.getState().selectHost(REMOTE)
    const remoteMod = await import('@/stores/presets')
    expect(localStorage.getItem(remoteMod.showLaunchBarStorageKey(hostMod.activeHostKey(REMOTE)))).toBeNull()
    expect(remoteMod.usePresetsStore.getState().showPresetsBar).toBe(true)
  })
})
