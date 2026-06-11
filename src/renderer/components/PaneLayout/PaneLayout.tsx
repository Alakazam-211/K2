import React, { useCallback } from 'react'
import { Mosaic, MosaicWindow } from 'react-mosaic-component'
import type { MosaicBranch, MosaicNode } from 'react-mosaic-component'
import { PaneGroupView } from './PaneGroupView'
import { useTabsStore } from '@/stores/tabs'
import 'react-mosaic-component/react-mosaic-component.css'

// Error boundary to catch react-dnd "two MultiBackends" errors
// that can happen transiently during mosaic tree structure changes.
// Limited to 3 retries to prevent infinite error-recovery loops.
class MosaicErrorBoundary extends React.Component<
  { children: React.ReactNode; onError?: () => void },
  { hasError: boolean; retryCount: number }
> {
  state = { hasError: false, retryCount: 0 }
  static getDerivedStateFromError(): { hasError: boolean } {
    return { hasError: true }
  }
  componentDidCatch(): void {
    if (this.state.retryCount < 3) {
      setTimeout(() => this.setState((s) => ({
        hasError: false,
        retryCount: s.retryCount + 1
      })), 200)
    }
  }
  render(): React.ReactNode {
    if (this.state.hasError) return null
    return this.props.children
  }
}

interface PaneLayoutProps {
  tabId: string
}

export function PaneLayout({ tabId }: PaneLayoutProps): React.JSX.Element | null {
  const tab = useTabsStore((s) => {
    const found = s.tabs.find((t) => t.id === tabId)
    if (found) return found
    for (const g of s.extraGroups) {
      const f = g.tabs.find((t) => t.id === tabId)
      if (f) return f
    }
    return undefined
  })
  const updateMosaicTree = useTabsStore((s) => s.updateMosaicTree)

  const handleChange = useCallback(
    (newTree: MosaicNode<string> | null) => {
      updateMosaicTree(tabId, newTree)
    },
    [tabId, updateMosaicTree]
  )

  if (!tab || tab.mosaicTree === null) {
    return (
      <div className="flex h-full w-full items-center justify-center text-[var(--color-text-muted)]">
        No terminal panes. Press Cmd+T to open a new tab.
      </div>
    )
  }

  // Single pane (leaf node) — render directly without Mosaic wrapper.
  // This avoids react-dnd "two MultiBackends" conflicts when multiple
  // tab groups are rendered simultaneously.
  if (typeof tab.mosaicTree === 'string') {
    return (
      <div className="h-full w-full">
        <PaneGroupView tabId={tabId} paneGroupId={tab.mosaicTree} />
      </div>
    )
  }

  // Multi-pane (split) — use Mosaic for drag-to-resize
  const renderTile = (paneGroupId: string, path: MosaicBranch[]): React.JSX.Element => {
    return (
      <MosaicWindow<string>
        path={path}
        title=""
        toolbarControls={<></>}
        createNode={() => ''}
        renderPreview={() => <div />}
      >
        <PaneGroupView tabId={tabId} paneGroupId={paneGroupId} />
      </MosaicWindow>
    )
  }

  return (
    <MosaicErrorBoundary>
      <div className="mosaic-dark h-full w-full">
        <Mosaic<string>
          key={tabId}
          renderTile={renderTile}
          value={tab.mosaicTree}
          onChange={handleChange}
          className=""
        />
      </div>
    </MosaicErrorBoundary>
  )
}
