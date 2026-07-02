// @vitest-environment jsdom
//
// Pinned-chat retention — retainer/portal mechanics. The load-bearing
// invariant pinned here: a retained AgentChatPane INSTANCE survives
// being moved between the hidden host and a tab slot (and back) with
// ZERO remounts — the portal container is a stable div that gets
// physically re-parented, never a changed createPortal target (which
// would remount). Plus: MRU cap eviction unmounts the least-recently-
// visited instance, pinned-to-top growth widens the cap, and the gate
// falls back to the inline pane when the workspace isn't exempt.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, cleanup, act, waitFor } from '@testing-library/react'
import { useEffect, useRef } from 'react'

const h = vi.hoisted(() => ({
  supported: { value: true },
  // Monotonic instance ids + a mount counter let the tests distinguish
  // "same instance moved" (id stable, mounts unchanged) from a remount.
  nextInstanceId: { value: 0 },
  mounts: { value: 0 },
  unmounts: { value: 0 },
}))

vi.mock('@/lib/server-capabilities', () => ({
  useServerSupports: () => h.supported.value,
  serverSupports: () => h.supported.value,
}))

// Minimal zustand stand-ins for the two stores the retainer reads.
// (The real modules drag in the settings/daemon fetch graphs.)
vi.mock('@/stores/active', async () => {
  const { create } = await import('zustand')
  const useActiveStore = create(() => ({ activeProjectIds: new Set<string>() }))
  return { useActiveStore }
})
vi.mock('@/stores/projects', async () => {
  const { create } = await import('zustand')
  const useProjectsStore = create(() => ({
    projects: [] as Array<{ id: string; path: string; manuallyActive: number }>,
  }))
  return { useProjectsStore }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
// Eager-boot seeding resolves agent display names through the daemon
// helper; keep it inert + deterministic here.
vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async (path: string) => `resolved-${path.split('/').pop()}`),
}))

// AgentChatPane probe. Reads the real TabVisibilityContext so the
// visibility-through-portal contract is exercised, not mocked.
vi.mock('./AgentChatPane', async () => {
  const { useIsTabVisible } = await import('@/contexts/TabVisibilityContext')
  return {
    AgentChatPane: (props: { projectPath: string; agentName: string }) => {
      const idRef = useRef<number | null>(null)
      if (idRef.current === null) idRef.current = ++h.nextInstanceId.value
      // eslint-disable-next-line react-hooks/rules-of-hooks
      useEffect(() => {
        h.mounts.value += 1
        return () => {
          h.unmounts.value += 1
        }
      }, [])
      return (
        <div
          data-agent-chat={props.projectPath}
          data-instance={idRef.current}
          data-visible={String(useIsTabVisible())}
        />
      )
    },
  }
})

import { PinnedChatRetainer, PinnedChatGate } from './PinnedChatRetainer'
import { useRetainedChatStore, resetRetainedChatStore } from '@/stores/retained-chat'
import { useActiveStore } from '@/stores/active'
import { useProjectsStore } from '@/stores/projects'

type AnyStore = { setState: (s: object) => void }

function setActive(...ids: string[]): void {
  ;(useActiveStore as unknown as AnyStore).setState({ activeProjectIds: new Set(ids) })
}
function setProjects(
  projects: Array<{ id: string; path: string; manuallyActive: number }>,
): void {
  ;(useProjectsStore as unknown as AnyStore).setState({ projects })
}

const visit = (projectId: string): void =>
  useRetainedChatStore.getState().recordVisit({
    projectId,
    projectPath: `/ws/${projectId}`,
    agentName: `agent-${projectId}`,
  })

const paneFor = (projectId: string): HTMLElement | null =>
  document.querySelector(`[data-agent-chat="/ws/${projectId}"]`)

const hiddenHost = (): HTMLElement =>
  document.querySelector('[data-pinned-chat-hidden-host]')!

// jsdom has no ResizeObserver; the retainer's slot-mirror effect needs
// one. The fake records observed targets and lets a test FIRE a resize
// with explicit contentRect dims.
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = []
  observed: Element[] = []
  private readonly cb: ResizeObserverCallback
  constructor(cb: ResizeObserverCallback) {
    this.cb = cb
    FakeResizeObserver.instances.push(this)
  }
  observe(el: Element): void {
    this.observed.push(el)
  }
  unobserve(el: Element): void {
    this.observed = this.observed.filter((o) => o !== el)
  }
  disconnect(): void {
    this.observed = []
  }
  fire(width: number, height: number): void {
    this.cb(
      [{ contentRect: { width, height } } as ResizeObserverEntry],
      this as unknown as ResizeObserver,
    )
  }
}

/** The live observer watching `el` — throws if the mirror isn't wired. */
const observerOf = (el: Element): FakeResizeObserver => {
  const found = FakeResizeObserver.instances.find((o) =>
    o.observed.includes(el),
  )
  if (!found) throw new Error('no ResizeObserver is observing the element')
  return found
}

/** Register a foreground slot for `projectId` whose box measures
 *  `width`×`height` (jsdom's getBoundingClientRect is all-zeros). */
function mountSlot(
  projectId: string,
  width: number,
  height: number,
): HTMLDivElement {
  const slotEl = document.createElement('div')
  slotEl.getBoundingClientRect = () =>
    ({ width, height, top: 0, left: 0, right: width, bottom: height, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect
  document.body.appendChild(slotEl)
  act(() => useRetainedChatStore.getState().registerSlot(projectId, slotEl))
  act(() => useRetainedChatStore.getState().setSlotVisible(projectId, true))
  return slotEl
}

beforeEach(() => {
  cleanup()
  document.body.innerHTML = ''
  resetRetainedChatStore()
  setActive()
  setProjects([])
  h.supported.value = true
  h.nextInstanceId.value = 0
  h.mounts.value = 0
  h.unmounts.value = 0
  FakeResizeObserver.instances = []
  vi.stubGlobal('ResizeObserver', FakeResizeObserver)
})

describe('portal-move — the same instance survives container moves', () => {
  it('visit → mounts in the hidden host; slot register/unregister MOVES the same instance (no remount)', () => {
    setActive('a')
    render(<PinnedChatRetainer />)
    act(() => visit('a'))

    // Mounted once, parked in the hidden host.
    let pane = paneFor('a')
    expect(pane).not.toBeNull()
    expect(hiddenHost().contains(pane)).toBe(true)
    expect(pane!.getAttribute('data-instance')).toBe('1')
    expect(pane!.getAttribute('data-visible')).toBe('false')
    expect(h.mounts.value).toBe(1)

    // Foreground: a slot registers → the SAME instance moves into it.
    const slotEl = document.createElement('div')
    document.body.appendChild(slotEl)
    act(() => useRetainedChatStore.getState().registerSlot('a', slotEl))
    act(() => useRetainedChatStore.getState().setSlotVisible('a', true))

    pane = paneFor('a')
    expect(slotEl.contains(pane)).toBe(true)
    expect(hiddenHost().contains(pane)).toBe(false)
    expect(pane!.getAttribute('data-instance')).toBe('1') // same instance
    expect(pane!.getAttribute('data-visible')).toBe('true')
    expect(h.mounts.value).toBe(1) // NO remount
    expect(h.unmounts.value).toBe(0)

    // Stash: slot unregisters → same instance parks back off-screen.
    act(() => useRetainedChatStore.getState().unregisterSlot('a', slotEl))
    pane = paneFor('a')
    expect(hiddenHost().contains(pane)).toBe(true)
    expect(pane!.getAttribute('data-instance')).toBe('1')
    expect(pane!.getAttribute('data-visible')).toBe('false')
    expect(h.mounts.value).toBe(1)
    expect(h.unmounts.value).toBe(0)
  })

  it('eviction unmounts the instance and removes its DOM', () => {
    setActive('a')
    render(<PinnedChatRetainer />)
    act(() => visit('a'))
    expect(paneFor('a')).not.toBeNull()

    act(() => useRetainedChatStore.getState().evict('a'))
    expect(paneFor('a')).toBeNull()
    expect(h.unmounts.value).toBe(1)
  })

  it('leaving the Active set detaches the retained pane', () => {
    setActive('a', 'b')
    render(<PinnedChatRetainer />)
    act(() => {
      visit('a')
      visit('b')
    })
    expect(paneFor('a')).not.toBeNull()

    // 'a' ages out / is dismissed — the canonical mirror shrinks.
    act(() => setActive('b'))
    expect(paneFor('a')).toBeNull()
    expect(paneFor('b')).not.toBeNull()
  })
})

// The workspace-switch zoom fix, retainer half: parked panes must
// measure TRUE content-area dims, so the hidden host tracks the
// FOREGROUND slot's box instead of staying a fixed 800×600. With an
// unchanged window, a background pane then re-foregrounds with
// identical dims — no resize, no re-fit.
describe('hidden host mirrors the foreground slot dims', () => {
  it('sizes the host to the visible slot box on register, then tracks its resizes', () => {
    setActive('a')
    render(<PinnedChatRetainer />)
    act(() => visit('a'))

    // Pre-first-measure fallback.
    expect(hiddenHost().style.width).toBe('800px')
    expect(hiddenHost().style.height).toBe('600px')

    // Foreground slot registers → host snaps to its measured box.
    const slotEl = mountSlot('a', 1280, 720)
    expect(hiddenHost().style.width).toBe('1280px')
    expect(hiddenHost().style.height).toBe('720px')

    // The slot's live resizes (window resize) track through.
    act(() => observerOf(slotEl).fire(1440, 900))
    expect(hiddenHost().style.width).toBe('1440px')
    expect(hiddenHost().style.height).toBe('900px')
  })

  it('keeps the last-known dims when the slot unregisters (workspace stashed)', () => {
    setActive('a')
    render(<PinnedChatRetainer />)
    act(() => visit('a'))
    const slotEl = mountSlot('a', 1280, 720)

    act(() => useRetainedChatStore.getState().unregisterSlot('a', slotEl))
    // No visible slot: the parked pane keeps measuring the last TRUE
    // foreground dims, not a reset 800×600.
    expect(hiddenHost().style.width).toBe('1280px')
    expect(hiddenHost().style.height).toBe('720px')
    // The mirror observer detached with the slot.
    expect(
      FakeResizeObserver.instances.every((o) => !o.observed.includes(slotEl)),
    ).toBe(true)
  })

  it('a zero-box measurement (hidden pane-item artifact) never shrinks the host', () => {
    setActive('a')
    render(<PinnedChatRetainer />)
    act(() => visit('a'))
    const slotEl = mountSlot('a', 1280, 720)

    act(() => observerOf(slotEl).fire(0, 0))
    expect(hiddenHost().style.width).toBe('1280px')
    expect(hiddenHost().style.height).toBe('720px')
  })

  it('re-registering after a switch re-mirrors the new slot box', () => {
    setActive('a', 'b')
    render(<PinnedChatRetainer />)
    act(() => {
      visit('a')
      visit('b')
    })
    const slotA = mountSlot('a', 1280, 720)
    act(() => useRetainedChatStore.getState().unregisterSlot('a', slotA))

    // The next foreground workspace's slot has a different box (e.g.
    // sidebar toggled while switching) — the host follows it.
    mountSlot('b', 1000, 640)
    expect(hiddenHost().style.width).toBe('1000px')
    expect(hiddenHost().style.height).toBe('640px')
  })
})

describe('MRU cap + pinned growth', () => {
  it('cap 5: the sixth visit evicts the least-recently-visited instance', () => {
    const ids = ['a', 'b', 'c', 'd', 'e', 'f']
    setActive(...ids)
    render(<PinnedChatRetainer />)
    // One commit per visit so each pane actually MOUNTS before the
    // overflow evicts the oldest (mirrors real sequential switching).
    for (const id of ids) act(() => visit(id))
    expect(paneFor('a')).toBeNull() // visited longest ago — evicted
    for (const id of ['b', 'c', 'd', 'e', 'f']) {
      expect(paneFor(id)).not.toBeNull()
    }
    // Eviction = unmount of exactly the overflow instance.
    expect(h.unmounts.value).toBe(1)
  })

  it('re-visiting rescues from eviction; the next-oldest becomes the victim', () => {
    const ids = ['a', 'b', 'c', 'd', 'e']
    setActive(...ids, 'f')
    render(<PinnedChatRetainer />)
    act(() => {
      for (const id of ids) visit(id)
    })
    act(() => visit('a')) // rescue
    act(() => visit('f')) // overflow — 'b' is now oldest
    expect(paneFor('b')).toBeNull()
    expect(paneFor('a')).not.toBeNull()
    expect(paneFor('f')).not.toBeNull()
  })

  it('cap grows to the pinned-to-top count: 6 pinned Active workspaces all retain', () => {
    const ids = ['a', 'b', 'c', 'd', 'e', 'f']
    setActive(...ids)
    setProjects(ids.map((id) => ({ id, path: `/ws/${id}`, manuallyActive: 1 })))
    render(<PinnedChatRetainer />)
    act(() => {
      for (const id of ids) visit(id)
    })
    for (const id of ids) {
      expect(paneFor(id)).not.toBeNull()
    }
    expect(h.unmounts.value).toBe(0)
  })
})

describe('eager boot attach + Active-membership tracking (slice 3)', () => {
  it('boot-seeds Active workspaces into the hidden host, pinned-to-top first, bounded by the cap', async () => {
    const ids = ['p1', 'p2', 'p3', 'p4', 'p5', 'p6', 'p7']
    setActive(...ids)
    // p6 is pinned to the top of the Active section — it must be seeded
    // ahead of the list order even though it comes later in projects.
    setProjects(
      ids.map((id) => ({ id, path: `/ws/${id}`, manuallyActive: id === 'p6' ? 1 : 0 })),
    )
    render(<PinnedChatRetainer />)

    await waitFor(() => expect(paneFor('p6')).not.toBeNull())
    const st = useRetainedChatStore.getState()
    expect(st.bootSeeded).toBe(true)
    // Cap 5 (1 pin ≤ base): pinned-first order p6,p1,p2,p3,p4 — never all 7.
    expect(st.mruOrder).toEqual(['p6', 'p1', 'p2', 'p3', 'p4'])
    for (const id of ['p6', 'p1', 'p2', 'p3', 'p4']) {
      const pane = paneFor(id)
      expect(pane).not.toBeNull()
      expect(hiddenHost().contains(pane)).toBe(true)
    }
    expect(paneFor('p5')).toBeNull()
    expect(paneFor('p7')).toBeNull()
    // Seeded entries carry the daemon-resolved agent name.
    expect(st.entries.get('p6')?.agentName).toBe('resolved-p6')
  })

  it('a real visit recorded before the seed lands stays in front of the seeds', async () => {
    setActive('fg', 'p1', 'p2')
    setProjects([
      { id: 'fg', path: '/ws/fg', manuallyActive: 0 },
      { id: 'p1', path: '/ws/p1', manuallyActive: 0 },
      { id: 'p2', path: '/ws/p2', manuallyActive: 0 },
    ])
    render(<PinnedChatRetainer />)
    act(() => visit('fg'))
    await waitFor(() =>
      expect(useRetainedChatStore.getState().bootSeeded).toBe(true),
    )
    expect(useRetainedChatStore.getState().mruOrder).toEqual(['fg', 'p1', 'p2'])
  })

  it('a workspace re-JOINING Active does not auto-attach (only boot/visit do)', async () => {
    setActive('a', 'b')
    setProjects([
      { id: 'a', path: '/ws/a', manuallyActive: 0 },
      { id: 'b', path: '/ws/b', manuallyActive: 0 },
    ])
    render(<PinnedChatRetainer />)
    await waitFor(() => expect(paneFor('a')).not.toBeNull())

    act(() => setActive('b')) // 'a' leaves — pruned + detached
    expect(paneFor('a')).toBeNull()

    act(() => setActive('a', 'b')) // 'a' re-joins — must stay detached
    expect(paneFor('a')).toBeNull()
    expect(useRetainedChatStore.getState().mruOrder).not.toContain('a')

    act(() => visit('a')) // a fresh visit re-attaches
    expect(paneFor('a')).not.toBeNull()
  })
})

describe('PinnedChatGate — exemption fallback', () => {
  it('exempt workspace renders a slot and the retainer portals the pane into it', () => {
    setActive('a')
    setProjects([{ id: 'a', path: '/ws/a', manuallyActive: 0 }])
    render(
      <>
        <PinnedChatRetainer />
        <PinnedChatGate agentName="agent-a" projectPath="/ws/a" />
      </>,
    )
    const slot = document.querySelector('[data-pinned-chat-slot="a"]')
    expect(slot).not.toBeNull()
    const pane = paneFor('a')
    expect(pane).not.toBeNull()
    expect(slot!.contains(pane)).toBe(true)
    expect(h.mounts.value).toBe(1)
  })

  it('non-Active workspace falls back to the inline pane (no slot, no retention)', () => {
    setActive() // 'a' not Active
    setProjects([{ id: 'a', path: '/ws/a', manuallyActive: 0 }])
    render(
      <>
        <PinnedChatRetainer />
        <PinnedChatGate agentName="agent-a" projectPath="/ws/a" />
      </>,
    )
    expect(document.querySelector('[data-pinned-chat-slot="a"]')).toBeNull()
    const pane = paneFor('a')
    expect(pane).not.toBeNull() // inline, today's path
    expect(hiddenHost().contains(pane)).toBe(false)
  })

  it('daemon without the pinned-chat capability falls back to the inline pane', () => {
    h.supported.value = false
    setActive('a')
    setProjects([{ id: 'a', path: '/ws/a', manuallyActive: 0 }])
    render(
      <>
        <PinnedChatRetainer />
        <PinnedChatGate agentName="agent-a" projectPath="/ws/a" />
      </>,
    )
    expect(document.querySelector('[data-pinned-chat-slot="a"]')).toBeNull()
    expect(paneFor('a')).not.toBeNull()
  })
})
