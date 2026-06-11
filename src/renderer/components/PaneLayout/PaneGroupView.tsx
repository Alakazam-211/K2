import { useCallback, useContext, useState } from 'react'
import { AlacrittyTerminalView } from '@/components/Terminal/AlacrittyTerminalView'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { FileViewerPane } from '@/components/FileViewerPane/FileViewerPane'
import { AgentPane } from '@/components/AgentPane/AgentPane'
import { useTabsStore } from '@/stores/tabs'
import type { TerminalItemData, FileViewerItemData, AgentItemData } from '@/stores/tabs'
import { useActiveAgentsStore, type ActiveAgent } from '@/stores/active-agents'
import AgentCloseDialog from '@/components/AgentCloseDialog/AgentCloseDialog'
import { PaneTabBar } from './PaneTabBar'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'

// ── Props ────────────────────────────────────────────────────────────────

interface PaneGroupViewProps {
  tabId: string
  paneGroupId: string
}

// ── Component ────────────────────────────────────────────────────────────

export function PaneGroupView({ tabId, paneGroupId }: PaneGroupViewProps): React.JSX.Element {
  const paneGroup = useTabsStore((s) => {
    let tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) {
      for (const g of s.extraGroups) {
        tab = g.tabs.find((t) => t.id === tabId)
        if (tab) break
      }
    }
    if (!tab || !tab.paneGroups) return undefined
    return tab.paneGroups.get(paneGroupId)
  })

  // 0.37.4 Phase B: read the parent tab's title once at render
  // time. When the title is non-default (i.e. set by ChatHistory
  // / openHeartbeatTab / restored layout), pass it down to
  // TerminalPane as `seedLabel` so the daemon stamps it as the
  // authoritative session label and locks the source — PTY title
  // events from `claude --resume` etc. can no longer overwrite.
  // Default `Terminal N` titles are filtered out so we don't
  // lock the label on a vanilla Cmd+T tab.
  const tabTitle = useTabsStore((s) => {
    let tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) {
      for (const g of s.extraGroups) {
        tab = g.tabs.find((t) => t.id === tabId)
        if (tab) break
      }
    }
    return tab?.title
  })
  const isMeaningfulTitle =
    !!tabTitle && !/^Terminal \d+$/.test(tabTitle) && tabTitle !== 'Untitled'

  const activateItem = useTabsStore((s) => s.activateItemInPaneGroup)
  const closeItem = useTabsStore((s) => s.closeItemInPaneGroup)
  const removePaneFromTab = useTabsStore((s) => s.removePaneFromTab)
  const removeTabFromGroup = useTabsStore((s) => s.removeTabFromGroup)

  // Check if this is a split (more than one pane in the mosaic tree)
  const hasSplits = useTabsStore((s) => {
    let tab = s.tabs.find((t) => t.id === tabId)
    if (!tab) {
      for (const g of s.extraGroups) {
        tab = g.tabs.find((t) => t.id === tabId)
        if (tab) break
      }
    }
    if (!tab || !tab.paneGroups) return false
    return tab.paneGroups.size > 1
  })

  const handleActivate = useCallback(
    (index: number) => {
      activateItem?.(tabId, paneGroupId, index)
    },
    [tabId, paneGroupId, activateItem]
  )

  const [pendingPaneClose, setPendingPaneClose] = useState<{
    itemId: string
    agents: ActiveAgent[]
  } | null>(null)

  // Track panes where the agent command exited — these fall back to a plain shell
  const [fallbackPanes, setFallbackPanes] = useState<Set<string>>(new Set())

  const handleClose = useCallback(
    (itemId: string) => {
      const item = paneGroup?.items.find(i => i.id === itemId)
      if (item?.type === 'terminal') {
        const data = item.data as TerminalItemData
        const agent = useActiveAgentsStore.getState().agents.get(data.terminalId)
        if (agent) {
          setPendingPaneClose({ itemId, agents: [agent] })
          return
        }
      }
      closeItem?.(tabId, paneGroupId, itemId)
    },
    [tabId, paneGroupId, closeItem, paneGroup]
  )

  const handleClosePane = useCallback(() => {
    removePaneFromTab(tabId, paneGroupId)
  }, [tabId, paneGroupId, removePaneFromTab])

  // ── Empty state ────────────────────────────────────────────────────────
  if (!paneGroup || !paneGroup.items || paneGroup.items.length === 0) {
    return (
      <div className="flex h-full w-full flex-col">
        <div
          className="flex items-center border-b border-[var(--color-border)]"
          style={{
            height: '24px',
            minHeight: '24px',
            background: '#111',
            fontSize: '11px',
            fontFamily: "'MesloLGM Nerd Font', Menlo, Monaco, monospace"
          }}
        />
        <div className="flex flex-1 items-center justify-center text-[var(--color-text-muted)]" style={{ fontSize: '11px' }}>
          Empty pane
        </div>
      </div>
    )
  }

  // ── Active item ────────────────────────────────────────────────────────
  const activeIndex = Math.min(paneGroup.activeItemIndex, paneGroup.items.length - 1)
  const activeItem = paneGroup.items[activeIndex]

  if (!activeItem) {
    return (
      <div className="flex h-full w-full flex-col">
        <PaneTabBar
          items={paneGroup.items}
          activeItemIndex={activeIndex}
          onActivate={handleActivate}
          onClose={handleClose}
          onClosePane={hasSplits ? handleClosePane : undefined}
        />
        <div className="flex flex-1 items-center justify-center text-[var(--color-text-muted)]" style={{ fontSize: '11px' }}>
          Empty pane
        </div>
      </div>
    )
  }

  // Only show per-pane tab bar when there are splits
  // (so the close-pane button is accessible).
  // Single-pane items are managed by the workspace TabBar at the top.
  const showPaneTabBar = hasSplits

  // Whether the enclosing tab is visible. PaneGroupView is nested inside
  // TerminalArea's tab wrapper, which provides `false` when the outer
  // tab is hidden. We AND this with per-item active state so nested
  // consumers (CodeEditor, xterm) only treat themselves as "visible"
  // when both the outer tab AND this specific pane item are active.
  const tabIsVisible = useContext(TabVisibilityContext)

  return (
    <>
      <div className="flex h-full w-full flex-col" data-pane-group-id={paneGroupId} data-tab-id={tabId}>
        {showPaneTabBar && (
          <PaneTabBar
            items={paneGroup.items}
            activeItemIndex={activeIndex}
            onActivate={handleActivate}
            onClose={handleClose}
            onClosePane={hasSplits ? handleClosePane : undefined}
            tabId={tabId}
            paneGroupId={paneGroupId}
          />
        )}

        <div className="flex-1 min-h-0 relative">
          {/*
            Retained-view model: every item in the paneGroup stays
            mounted so scroll/cursor/focus state lives on the DOM across
            pane switches. Only the active item is visible; others are
            hidden with display:none.
          */}
          {paneGroup.items.map((item, index) => {
            const isActiveItem = index === activeIndex
            const itemIsVisible = tabIsVisible && isActiveItem
            const hidden = !isActiveItem

            let content: React.ReactNode = null
            if (item.type === 'terminal') {
              const raw = item.data as TerminalItemData
              const isFallback = fallbackPanes.has(item.id)
              const td = isFallback
                ? { ...raw, command: undefined as string | undefined, args: undefined as string[] | undefined, terminalId: `${raw.terminalId}-shell` }
                : raw
              // Phase 4.5: dispatch to the renderer this tab was
              // created with. A missing `renderer` field is treated
              // as 'alacritty' — preserves behavior for every tab
              // that existed before the toggle shipped. The
              // preference for NEW tabs is stamped at
              // makeTerminalPaneGroup time; mid-session toggle
              // changes don't affect already-open terminals.
              // In dev, loudly surface when a terminal item lacks a
              // renderer field — historical bug where require() in an
              // ESM bundle silently threw and every tab fell through
              // to 'alacritty'. If this fires, some tab-creation
              // path is bypassing makeTerminalPaneGroup /
              // paneDataToItem — that path needs currentRenderer()
              // added to it.
              if (import.meta.env.DEV && raw.renderer === undefined) {
                // eslint-disable-next-line no-console
                console.warn(
                  '[tabs] terminal item has no renderer field; defaulting to alacritty',
                  { terminalId: td.terminalId, cwd: td.cwd },
                )
              }
              if (raw.renderer === 'alacritty-v2') {
                // A5: daemon-hosted v2 thin client.
                // See .k2so/prds/alacritty-v2.md.
                content = (
                  <TerminalPane
                    terminalId={td.terminalId}
                    tabId={tabId}
                    paneGroupId={paneGroupId}
                    cwd={td.cwd}
                    command={td.command}
                    args={td.args}
                    spawnedAt={td.spawnedAt}
                    attachAgentName={(td as any).attachAgentName}
                    // 0.37.4 Phase B: seed + lock the daemon label
                    // when the tab has a meaningful title. Stops
                    // claude --resume's "Claude Code" title from
                    // smudging chat-history-restored tabs.
                    seedLabel={isMeaningfulTitle ? tabTitle : undefined}
                    lockLabel={isMeaningfulTitle ? true : undefined}
                  />
                )
              } else {
                content = (
                <AlacrittyTerminalView
                  terminalId={td.terminalId}
                  tabId={tabId}
                  paneGroupId={paneGroupId}
                  cwd={td.cwd}
                  command={td.command}
                  args={td.args}
                  spawnedAt={td.spawnedAt}
                  onExit={(exitCode) => {
                    const hadCommand = raw.command
                    if (hadCommand && !isFallback) {
                      setFallbackPanes((prev) => new Set(prev).add(item.id))
                    } else if (exitCode === 127) {
                      const store = useTabsStore.getState()
                      const groupIdx = store.tabs.some((t) => t.id === tabId)
                        ? 0
                        : store.extraGroups.findIndex((g) => g.tabs.some((t) => t.id === tabId)) + 1
                      if (groupIdx >= 0) {
                        removeTabFromGroup(groupIdx, tabId)
                      }
                    } else if (exitCode === 0) {
                      handleClose(item.id)
                    }
                  }}
                />
                )
              }
            } else if (item.type === 'file-viewer') {
              const fd = item.data as FileViewerItemData
              content = (
                <FileViewerPane
                  filePath={fd.filePath}
                  mode={fd.mode}
                  paneId={item.id}
                  paneGroupId={paneGroupId}
                  tabId={tabId}
                  initialScrollTop={fd.scrollTop}
                  initialCursorPos={fd.cursorPos}
                  onClose={() => handleClose(item.id)}
                />
              )
            } else if (item.type === 'agent') {
              const ad = item.data as AgentItemData
              content = (
                <AgentPane
                  agentName={ad.agentName}
                  projectPath={ad.projectPath}
                  section={ad.section}
                  restoredSessionId={ad.sessionId}
                  onClose={() => handleClose(item.id)}
                />
              )
            }

            return (
              <TabVisibilityContext.Provider key={item.id} value={itemIsVisible}>
                <div
                  className="absolute inset-0"
                  style={{ display: hidden ? 'none' : 'block' }}
                  aria-hidden={hidden}
                  data-pane-item-id={item.id}
                >
                  {content}
                </div>
              </TabVisibilityContext.Provider>
            )
          })}
        </div>
      </div>

      {pendingPaneClose && (
        <AgentCloseDialog
          agents={pendingPaneClose.agents}
          mode="tab"
          onConfirm={() => {
            closeItem?.(tabId, paneGroupId, pendingPaneClose.itemId)
            setPendingPaneClose(null)
          }}
          onCancel={() => setPendingPaneClose(null)}
        />
      )}
    </>
  )
}
