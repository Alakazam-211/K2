// @vitest-environment jsdom
// 0.46.0 (Rosson 2026-10-16): the right drawer offers Chats and Agent; the
// Changes tab is gone (its panel stays in the repo, not offered here).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/components/AgentPanel/AgentPanel', () => ({ default: () => <div data-testid="drawer-agent" /> }))
vi.mock('@/components/ChatHistory/ChatHistory', () => ({ default: () => <div data-testid="drawer-history" /> }))
vi.mock('@/components/FileTree/FileTree', () => ({ default: () => <div data-testid="drawer-files" /> }))
vi.mock('@/components/WorkspacePanel/WorkspacePanel', () => ({ default: () => <div data-testid="drawer-workspace" /> }))

import { RightPanelContent } from './WorkspaceDrawers'
import { usePanelsStore } from '@/stores/panels'

beforeEach(() => {
  usePanelsStore.setState({
    rightPanelOpen: true,
    rightPanelTabs: usePanelsStore.getInitialState().rightPanelTabs,
    rightPanelActiveTab: 'agent',
  })
})

afterEach(() => {
  cleanup()
})

describe('right drawer', () => {
  it('offers Chats and Agent, not Changes', () => {
    render(<RightPanelContent rootPath="/ws" />)
    expect(screen.getByText('Chats')).toBeTruthy()
    expect(screen.getByText('Agent')).toBeTruthy()
    expect(screen.queryByText('Changes')).toBeNull()
  })

  it('the Agent tab renders the Agent panel', () => {
    render(<RightPanelContent rootPath="/ws" />)
    expect(screen.getByTestId('drawer-agent')).toBeTruthy()
    expect(screen.queryByTestId('drawer-history')).toBeNull()
  })
})
