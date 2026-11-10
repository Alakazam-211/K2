// @vitest-environment jsdom
// Thread view's floating objects: Thread view only, every control an object
// of its own, and the pane told how much room they take.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'
import { useRef, type RefObject } from 'react'

vi.mock('@/components/Thread/ThreadSenderAvatars', () => ({
  AgentAvatar: () => <span data-testid="avatar" />,
}))

import { renderInRoom, testRoom } from '@/test-utils/room'
import { primaryScope } from '@/kessel/server-scope'
import { SidecarSessionHeader } from './AgentSessionChrome'
import { ThreadFloatBell } from './ThreadFloatBell'
import {
  THREAD_FLOATS_HOST_ATTR,
  THREAD_FLOAT_INSET_VAR,
  THREAD_FLOAT_LEFT_VAR,
  THREAD_FLOAT_TOP_VAR,
  floatProps,
  useThreadFloats,
  useThreadFloatsClearOfTerminal,
} from './threadFloats'
import type { SessionViewTab } from './sessionViewTab'

afterEach(cleanup)

const room = testRoom({ tabs: {}, scope: primaryScope() })

function header(viewTab: SessionViewTab) {
  return (
    <div data-testid="pane">
      <SidecarSessionHeader
        title="sales/reviewer"
        addr="sales/reviewer"
        terminalRevealed={false}
        onToggleTerminal={() => {}}
        terminalAvailable
        viewTab={viewTab}
        splitLeft="terminal"
        splitRight="thread"
        onViewTabChange={() => {}}
        onSplitLeft={() => {}}
        onSplitRight={() => {}}
        onRefresh={() => {}}
        refreshing={false}
        bell={<button type="button" data-testid="bell-stub" />}
      />
    </div>
  )
}

describe('floatProps', () => {
  it('marks a floating control and chamfers it, and leaves others alone', () => {
    expect(floatProps(true, 'round')).toEqual({
      'data-k2-float': 'round',
      'data-k2-chamfer': 'control',
      'data-material': 'frost',
      'data-frost-shape': 'round',
    })
    expect(floatProps(true, 'name')['data-frost-shape']).toBe('pill')
    expect(floatProps(false, 'pill')).toEqual({})
  })
})

describe('the header', () => {
  it('floats in Thread view, with no bar and each control its own object', () => {
    renderInRoom(room, header('thread'))
    const h = screen.getByTestId('sidecar-session-header')
    expect(h.hasAttribute('data-thread-floats')).toBe(true)
    expect(h.className).not.toContain('border-b')
    expect(screen.getByTestId('session-view-button').getAttribute('data-k2-float')).toBe('pill')
    expect(screen.getByTestId('thread-terminal-toggle').getAttribute('data-k2-float')).toBe('round')
    expect(screen.getByRole('button', { name: 'Refresh session' }).getAttribute('data-k2-float')).toBe('round')
    expect(screen.getByTestId('thread-float-scrim')).not.toBeNull()
    // Still finds its pane the way the reveal chord and the identity do.
    expect(h.parentElement?.getAttribute('data-testid')).toBe('pane')
  })

  it('has the search object on the left, right after the bell', () => {
    renderInRoom(room, header('thread'))
    const search = screen.getByTestId('thread-search-button')
    expect(search.getAttribute('data-k2-float')).toBe('round')
    expect(search.closest('[data-testid="thread-search"]')?.previousElementSibling?.getAttribute('data-testid')).toBe('bell-stub')
  })

  it('stays a bar in every other view', () => {
    for (const view of ['terminal', 'split', 'chatter'] as const) {
      renderInRoom(room, header(view))
      const h = screen.getByTestId('sidecar-session-header')
      expect(h.hasAttribute('data-thread-floats')).toBe(false)
      expect(h.className).toContain('border-b')
      expect(screen.getByTestId('session-view-button').hasAttribute('data-k2-float')).toBe(false)
      expect(screen.queryByTestId('thread-float-scrim')).toBeNull()
      expect(screen.queryByTestId('thread-search-button')).toBeNull()
      cleanup()
    }
  })
})

describe('the bell', () => {
  const bellRoom = testRoom({
    tabs: {},
    scope: primaryScope(),
    projects: [
      { id: 'p1', path: '/ws/one', completionSoundEnabled: 1, workspaces: [] } as never,
      { id: 'p2', path: '/ws/two', completionSoundEnabled: 0, workspaces: [{ worktreePath: '/ws/two-wt' }] } as never,
    ],
  })
  const bell = (): Promise<HTMLElement> => screen.findByTestId('workspace-completion-bell')

  it('is the workspace bell as a round object, on or off from the same setting', async () => {
    renderInRoom(bellRoom, <ThreadFloatBell projectId="p1" />)
    expect((await bell()).getAttribute('data-k2-float')).toBe('round')
    expect((await bell()).getAttribute('aria-checked')).toBe('true')
    cleanup()
    renderInRoom(bellRoom, <ThreadFloatBell projectId="p2" />)
    expect((await bell()).getAttribute('aria-checked')).toBe('false')
  })

  it('finds a tab by its cwd, a worktree included, and shows nothing without a project', async () => {
    renderInRoom(bellRoom, <ThreadFloatBell cwd="/ws/two-wt" />)
    expect((await bell()).getAttribute('aria-checked')).toBe('false')
    cleanup()
    renderInRoom(bellRoom, <ThreadFloatBell cwd="/elsewhere" />)
    expect(screen.queryByTestId('workspace-completion-bell')).toBeNull()
  })
})

describe('useThreadFloats', () => {
  function Header({ floating }: { floating: boolean }) {
    const ref = useThreadFloats(floating)
    return <div ref={ref} data-testid="floats" />
  }

  it('publishes the height on the parent while floating and clears it after', () => {
    const { rerender } = render(
      <div data-testid="host">
        <Header floating />
      </div>,
    )
    const host = screen.getByTestId('host')
    expect(host.hasAttribute(THREAD_FLOATS_HOST_ATTR)).toBe(true)
    expect(host.style.getPropertyValue(THREAD_FLOAT_INSET_VAR)).toBe('0px')
    rerender(
      <div data-testid="host">
        <Header floating={false} />
      </div>,
    )
    expect(host.hasAttribute(THREAD_FLOATS_HOST_ATTR)).toBe(false)
    expect(host.style.getPropertyValue(THREAD_FLOAT_INSET_VAR)).toBe('')
  })
})

describe('useThreadFloatsClearOfTerminal', () => {
  function Reveal({ active, left, top = 0 }: { active: boolean; left: number; top?: number }) {
    const ref = useRef<HTMLDivElement>(null) as RefObject<HTMLDivElement | null>
    useThreadFloatsClearOfTerminal(ref, active, { left, top })
    return <div ref={ref} />
  }

  it('moves the floats beside the terminal on the host, and only there', () => {
    const { rerender } = render(
      <div data-testid="host" {...{ [THREAD_FLOATS_HOST_ATTR]: '' }}>
        <Reveal active left={420} />
      </div>,
    )
    const host = screen.getByTestId('host')
    expect(host.style.getPropertyValue(THREAD_FLOAT_LEFT_VAR)).toBe('420px')
    expect(host.style.getPropertyValue(THREAD_FLOAT_TOP_VAR)).toBe('0px')
    rerender(
      <div data-testid="host" {...{ [THREAD_FLOATS_HOST_ATTR]: '' }}>
        <Reveal active left={0} top={324} />
      </div>,
    )
    expect(host.style.getPropertyValue(THREAD_FLOAT_LEFT_VAR)).toBe('0px')
    expect(host.style.getPropertyValue(THREAD_FLOAT_TOP_VAR)).toBe('324px')
  })

  it('does nothing outside Thread view or without a host', () => {
    const { rerender } = render(
      <div data-testid="host" {...{ [THREAD_FLOATS_HOST_ATTR]: '' }}>
        <Reveal active={false} left={420} />
      </div>,
    )
    expect(screen.getByTestId('host').style.getPropertyValue(THREAD_FLOAT_LEFT_VAR)).toBe('')
    rerender(
      <div data-testid="no-host">
        <Reveal active left={420} />
      </div>,
    )
    expect(screen.getByTestId('no-host').style.getPropertyValue(THREAD_FLOAT_LEFT_VAR)).toBe('')
  })
})
