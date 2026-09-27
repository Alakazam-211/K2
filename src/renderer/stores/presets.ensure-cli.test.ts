// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import * as daemonCli from '@/lib/daemon-cli'
import { useToastStore } from '@/stores/toast'
import { usePresetsStore, type AgentPreset } from '@/stores/presets'
import { useTabsStore } from '@/stores/tabs'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

function preset(over: Partial<AgentPreset> & Pick<AgentPreset, 'id' | 'label' | 'command'>): AgentPreset {
  return {
    icon: null,
    enabled: 1,
    sortOrder: 0,
    isBuiltIn: 1,
    createdAt: 1,
    ...over,
  }
}

describe('launchPreset ensure-cli', () => {
  const realAddTabToGroup = useTabsStore.getState().addTabToGroup

  beforeEach(() => {
    useToastStore.setState({ toasts: [] })
    usePresetsStore.setState({ presets: [], showPresetsBar: true })
    useTabsStore.setState({
      tabs: [],
      activeTabId: null,
      splitCount: 1,
      extraGroups: [],
      activeGroupIndex: 0,
      addTabToGroup: realAddTabToGroup,
    })
  })

  afterEach(() => {
    vi.restoreAllMocks()
    useTabsStore.setState({ addTabToGroup: realAddTabToGroup })
    useToastStore.setState({ toasts: [] })
  })

  it('does not call addTabToGroup when ensure-cli fails', async () => {
    const addTabToGroup = vi.fn()
    useTabsStore.setState({ addTabToGroup })
    usePresetsStore.setState({
      presets: [preset({ id: 'g', label: 'Grok', command: 'grok --always-approve' })],
    })
    vi.spyOn(daemonCli, 'daemonCliPost').mockRejectedValue(new Error('grok installed but not on PATH'))
    await usePresetsStore.getState().launchPreset('g', '/tmp/proj', 'tab')
    expect(daemonCli.daemonCliPost).toHaveBeenCalledWith('agents/ensure-cli', { program: 'grok' })
    expect(addTabToGroup).not.toHaveBeenCalled()
    expect(useTabsStore.getState().tabs).toHaveLength(0)
    expect(useToastStore.getState().toasts.map((t) => t.message)).toContain(
      'grok installed but not on PATH',
    )
  })

  it('does not install a path token and still opens the tab', async () => {
    const post = vi.spyOn(daemonCli, 'daemonCliPost')
    usePresetsStore.setState({
      presets: [preset({ id: 'p', label: 'Claude', command: '/usr/bin/claude --yolo' })],
    })
    await usePresetsStore.getState().launchPreset('p', '/tmp/proj', 'tab')
    expect(post).not.toHaveBeenCalled()
    expect(useTabsStore.getState().tabs).toHaveLength(1)
    expect(useTabsStore.getState().tabs[0]?.title).toBe('Claude')
  })
})
