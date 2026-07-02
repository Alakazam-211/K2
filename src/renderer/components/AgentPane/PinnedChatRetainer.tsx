// Pinned-chat background retention — the per-window retainer.
// Design: .k2/notes/pinned-chat-background-render-design.md §2.2/§2.3.
//
// The load-bearing fact: workspace switching FULLY UNMOUNTS the outgoing
// workspace's panes (`stashWorkspace` sets tabs: []), so "keep the
// grid-WS while hidden" alone cannot span workspaces — the component
// that owned the socket is gone. The retainer moves OWNERSHIP of the
// pinned-chat React instance out of the per-workspace tab tree into a
// per-window host that survives switches:
//
//   - `PinnedChatRetainer` (mounted once in App, all layout branches at
//     the same child position so branch switches never remount it) owns
//     one `AgentChatPane` instance per retained workspace.
//   - Each instance renders through `createPortal` into a STABLE
//     per-workspace container div that is physically re-parented
//     (appendChild) between the foreground tab's `PinnedChatSlot` and an
//     off-screen hidden host. The portal's container prop never changes
//     — React would REMOUNT the children if it did (reconcileSinglePortal
//     keys on containerInfo; the design note's "changing a portal's
//     container moves the DOM" is wrong for naive createPortal) — so
//     state, grid-WS, scroll offset and snapshot all survive; re-show is
//     literally the same DOM appearing in the slot.
//   - `PinnedChatGate` is what the pinned Chat tab renders (AgentPane
//     dispatches here): a slot when the workspace is exempt, or today's
//     inline `AgentChatPane` otherwise (legacy daemon / non-Active /
//     kill switch) — byte-identical fallback.
//
// Retained-set policy (owner decisions, pure module
// kessel-term/retainedChat.ts): MRU by visit, cap max(5, pinned-to-top
// count), Active-membership required, host switch drops everything.

import React, { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AgentChatPane } from './AgentChatPane'
import { TabVisibilityContext, useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { useActiveStore } from '@/stores/active'
import { useProjectsStore } from '@/stores/projects'
import { useServerSupports } from '@/lib/server-capabilities'
import {
  useRetainedChatStore,
  type RetainedChatEntry,
} from '@/stores/retained-chat'
import { computeRetainedSet, retainedCap } from '@/kessel-term/retainedChat'
import { agentDisplayName } from '@/lib/workspace-agent'

/** Escape hatch: `localStorage.K2SO_PINNED_RETAIN_OFF = '1'` restores
 *  park-on-hidden everywhere without a rebuild (read per render — flip
 *  takes effect on the next workspace switch). */
export function retentionDisabled(): boolean {
  try {
    return (
      typeof localStorage !== 'undefined' &&
      localStorage.getItem('K2SO_PINNED_RETAIN_OFF') === '1'
    )
  } catch {
    return false
  }
}

interface PinnedChatGateProps {
  agentName: string
  projectPath: string
  restoredSessionId?: string
}

/**
 * What the pinned Chat tab's pane item renders. Exemption predicate
 * (design §2.1): daemon-owned pinned chat (capability) ∧ workspace in
 * the canonical Active section ∧ retention not killed. Exempt → an
 * empty slot the retainer portals the retained instance into.
 * Not exempt → inline `AgentChatPane`, exactly today's path.
 */
export function PinnedChatGate({
  agentName,
  projectPath,
  restoredSessionId,
}: PinnedChatGateProps): React.JSX.Element {
  const projectId = useProjectsStore(
    (s) => s.projects.find((p) => p.path === projectPath)?.id ?? null,
  )
  const daemonOwnsChat = useServerSupports('daemon-pinned-chat')
  const isActive = useActiveStore(
    (s) => projectId !== null && s.activeProjectIds.has(projectId),
  )
  const exempt =
    daemonOwnsChat && projectId !== null && isActive && !retentionDisabled()

  if (!exempt || projectId === null) {
    return (
      <AgentChatPane
        agentName={agentName}
        projectPath={projectPath}
        restoredSessionId={restoredSessionId}
      />
    )
  }
  return (
    <PinnedChatSlot
      projectId={projectId}
      agentName={agentName}
      projectPath={projectPath}
      restoredSessionId={restoredSessionId}
    />
  )
}

interface PinnedChatSlotProps extends PinnedChatGateProps {
  projectId: string
}

/**
 * The portal target living in the tab tree. Registers its element +
 * visibility with the retained-chat store; mounting one IS the "visit"
 * gesture (a workspace is foregrounded ⇒ its tab tree — including the
 * pinned chat tab, even when a sibling tab is active — is mounted).
 * Unregisters on stash; the retained instance survives, re-parented to
 * the hidden host.
 */
function PinnedChatSlot({
  projectId,
  agentName,
  projectPath,
  restoredSessionId,
}: PinnedChatSlotProps): React.JSX.Element {
  const visible = useIsTabVisible()
  const elRef = useRef<HTMLDivElement | null>(null)

  useLayoutEffect(() => {
    const el = elRef.current
    if (!el) return
    useRetainedChatStore.getState().registerSlot(projectId, el)
    return () => {
      useRetainedChatStore.getState().unregisterSlot(projectId, el)
    }
  }, [projectId])

  // Declared AFTER registerSlot so the slot exists when visibility lands.
  useLayoutEffect(() => {
    useRetainedChatStore.getState().setSlotVisible(projectId, visible)
  }, [projectId, visible])

  // Visit = this workspace's exempt pinned chat was foregrounded.
  // Re-runs on prop churn only to keep the entry's props current
  // (recordVisit is an MRU move-to-front; the foreground workspace is
  // already at the front, so a late agentName resolve can't reorder).
  useEffect(() => {
    useRetainedChatStore.getState().recordVisit({
      projectId,
      projectPath,
      agentName,
      restoredSessionId,
    })
  }, [projectId, projectPath, agentName, restoredSessionId])

  return (
    <div
      ref={elRef}
      className="h-full w-full"
      data-pinned-chat-slot={projectId}
    />
  )
}

/**
 * Per-window host for the retained instances. Mounted once in App —
 * as the FIRST child of every top-level layout branch (default / focus
 * / settings), so React's index-based reconciliation keeps this
 * instance (and every retained pane under it) alive across layout-mode
 * switches. Host switches unmount it via `<App key={hostKey}>` and the
 * store resets via onActiveHostChange — background attachments never
 * span hosts.
 */
export function PinnedChatRetainer(): React.JSX.Element | null {
  const daemonOwnsChat = useServerSupports('daemon-pinned-chat')
  const entries = useRetainedChatStore((s) => s.entries)
  const mruOrder = useRetainedChatStore((s) => s.mruOrder)
  const slots = useRetainedChatStore((s) => s.slots)
  const activeIds = useActiveStore((s) => s.activeProjectIds)
  const projects = useProjectsStore((s) => s.projects)
  // State (not ref): the first retained entry can arrive before the
  // hidden host exists; the setState re-render re-parents it in.
  const [hiddenHost, setHiddenHost] = useState<HTMLDivElement | null>(null)

  // Cap growth input: workspaces pinned to the top of the Active
  // section (ActiveBar's sortPinnedFirst partition = manuallyActive ∩
  // canonical Active).
  const pinnedToTopCount = useMemo(
    () =>
      projects.filter((p) => p.manuallyActive !== 0 && activeIds.has(p.id))
        .length,
    [projects, activeIds],
  )

  const retained = useMemo(
    () =>
      daemonOwnsChat && !retentionDisabled()
        ? computeRetainedSet({
            mruOrder,
            activeProjectIds: activeIds,
            pinnedToTopCount,
          })
        : [],
    [daemonOwnsChat, mruOrder, activeIds, pinnedToTopCount],
  )

  // Active-section membership tracking: a workspace LEAVING the
  // canonical Active set is dropped from the visit order (its instance
  // already detached via computeRetainedSet's filter), so a later
  // re-JOIN does not auto-attach — only boot seeding or a fresh visit
  // does (owner decision). Boot-transient safe: an empty mirror can
  // only prune an order that visits/seeds (both Active-gated) have not
  // populated yet.
  useEffect(() => {
    useRetainedChatStore.getState().pruneToActive(activeIds)
  }, [activeIds])

  // Eager boot attach (owner decision 2): once per host session, when
  // the Active mirror + project list have landed, pre-attach the pinned
  // chats of Active-section workspaces — seed MRU order = Active-list
  // order (pinned-to-top first, then projects order — ActiveBar's
  // sortPinnedFirst partition), bounded by the cap, never all
  // workspaces. Seeded instances mount into the hidden host and pay the
  // ensure/attach chain at boot so even the FIRST visit is instant.
  const bootSeeded = useRetainedChatStore((s) => s.bootSeeded)
  const seedStartedRef = useRef(false)
  useEffect(() => {
    if (bootSeeded || seedStartedRef.current) return
    if (!daemonOwnsChat || retentionDisabled()) return
    if (activeIds.size === 0 || projects.length === 0) return
    seedStartedRef.current = true

    const activeProjects = projects.filter((p) => activeIds.has(p.id))
    const ordered = [
      ...activeProjects.filter((p) => p.manuallyActive !== 0),
      ...activeProjects.filter((p) => p.manuallyActive === 0),
    ]
    const cap = retainedCap(pinnedToTopCount)
    const candidates = ordered.slice(0, cap)
    if (candidates.length === 0) return

    let cancelled = false
    void Promise.all(
      candidates.map(async (p): Promise<RetainedChatEntry> => ({
        projectId: p.id,
        projectPath: p.path,
        // Same resolution ladder as ensurePinnedAgentTabForMode's
        // fallback: the daemon's display-name helper is total; the
        // path basename is the never-empty last resort. (agentChatId
        // ignores the agent name — projectId alone is the canonical
        // identity — so a later slot visit with the tab's resolved
        // name upserts props without changing the instance.)
        agentName:
          (await agentDisplayName(p.path).catch(() => '')) ||
          (p.path.split('/').filter(Boolean).pop() ?? 'agent'),
      })),
    ).then((entries) => {
      if (cancelled) return
      // seedBoot is one-shot store-side too, so a remount that re-runs
      // this effect mid-flight cannot double-seed.
      useRetainedChatStore.getState().seedBoot(entries, cap)
    })
    return () => {
      cancelled = true
    }
  }, [bootSeeded, daemonOwnsChat, activeIds, projects, pinnedToTopCount])

  return (
    <>
      {/* Off-screen host for hidden retained panes — real dimensions so
          the grid keeps a valid layout while hidden (same trick as
          BackgroundTerminalSpawner). display:none is NOT used: the pane
          is deliberately kept rendering (viewport-windowed + memoized
          rows ⇒ cheap), which keeps activity detection live for
          background Active workspaces. Resize safety while parked here:
          the daemon only accepts resizes from the ACTIVE subscriber,
          and a hidden pane never claims active. */}
      <div
        ref={setHiddenHost}
        aria-hidden
        data-pinned-chat-hidden-host
        style={{
          position: 'fixed',
          top: -9999,
          left: -9999,
          width: 800,
          height: 600,
          overflow: 'hidden',
          opacity: 0,
          pointerEvents: 'none',
        }}
      />
      {retained.map((projectId) => {
        const entry = entries.get(projectId)
        if (!entry) return null
        const slot = slots.get(projectId) ?? null
        return (
          <RetainedPaneBoundary key={projectId} projectId={projectId}>
            <RetainedPane
              entry={entry}
              slotEl={slot?.el ?? null}
              slotVisible={slot?.visible === true}
              hiddenHost={hiddenHost}
            />
          </RetainedPaneBoundary>
        )
      })}
    </>
  )
}

interface RetainedPaneProps {
  entry: RetainedChatEntry
  slotEl: HTMLElement | null
  slotVisible: boolean
  hiddenHost: HTMLElement | null
}

function RetainedPane({
  entry,
  slotEl,
  slotVisible,
  hiddenHost,
}: RetainedPaneProps): React.ReactPortal {
  // The STABLE portal container — created once per retained instance,
  // physically re-parented between the slot and the hidden host. The
  // createPortal container argument never changes identity, so React
  // never remounts the pane across moves (the invariant the jsdom
  // portal-move test pins).
  const containerRef = useRef<HTMLDivElement | null>(null)
  if (containerRef.current === null) {
    const el = document.createElement('div')
    el.style.width = '100%'
    el.style.height = '100%'
    el.dataset.retainedChat = entry.projectId
    containerRef.current = el
  }

  useLayoutEffect(() => {
    const el = containerRef.current
    if (!el) return
    const host = slotEl ?? hiddenHost
    if (host && el.parentElement !== host) host.appendChild(el)
  }, [slotEl, hiddenHost])

  // Final removal on real unmount only (eviction / retainer teardown);
  // the re-parent effect above must never detach as a cleanup side
  // effect or every move would drop the DOM mid-commit.
  useLayoutEffect(() => {
    return () => {
      containerRef.current?.remove()
    }
  }, [])

  return createPortal(
    <TabVisibilityContext.Provider value={slotVisible && slotEl !== null}>
      <AgentChatPane
        agentName={entry.agentName}
        projectPath={entry.projectPath}
        restoredSessionId={entry.restoredSessionId}
        onDaemonSessionRemoved={() => {
          // Evict only when BACKGROUNDED (no slot): the next visit then
          // remounts fresh and ensure-pinned-chat find-or-spawns —
          // today's revisit behavior. A foreground pane (slot present)
          // stays mounted showing its idle/Retry state, exactly like
          // the non-retained pane does; evicting it would orphan the
          // slot (its visit already happened) and change behavior.
          const store = useRetainedChatStore.getState()
          if (!store.slots.has(entry.projectId)) store.evict(entry.projectId)
        }}
      />
    </TabVisibilityContext.Provider>,
    containerRef.current,
  )
}

/** Crash containment: a retained pane crash must not take down the
 *  retainer (and every other retained chat) with it. Pattern:
 *  FocusErrorBoundary (App.tsx). Renders nothing on error — the slot
 *  shows empty until the workspace is re-visited after eviction. */
class RetainedPaneBoundary extends React.Component<
  { projectId: string; children: React.ReactNode },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }
  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }
  componentDidCatch(error: Error, info: React.ErrorInfo): void {
    console.error(
      `[pinned-retain] retained pane crashed (project ${this.props.projectId}):`,
      error,
      info.componentStack,
    )
    // Drop the broken instance from the retained set so the next visit
    // starts clean instead of re-rendering the crashed subtree forever.
    useRetainedChatStore.getState().evict(this.props.projectId)
  }
  render(): React.ReactNode {
    if (this.state.error) return null
    return this.props.children
  }
}
