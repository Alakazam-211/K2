// The shape tree a Try shows (contract-sources-v1 §4.3, §8): one line per
// node, `pointer · type · sample`; arrays show their first 3 items. A click
// copies the node's JSON Pointer (the path syntax extract/rules use).
// Owner: B2. The pick-into-form picker is 0.45.7.

import React, { useState } from 'react'
import type { SourceTryResponse, TryTreeNode } from '@/lib/sources/sources-types'
import { muted, sampleText } from './data-sources-ui'

function depth(pointer: string): number {
  return pointer === '' ? 0 : pointer.split('/').length - 1
}

export function DataSourceTryTree({ nodes, truncated }: { nodes: TryTreeNode[]; truncated: boolean }): React.JSX.Element {
  const [copied, setCopied] = useState<string | null>(null)
  if (nodes.length === 0) return <p className={`text-[11px] ${muted}`}>No shape to show.</p>
  return (
    <div className="font-mono text-[11px] max-h-72 overflow-auto border border-[var(--color-border)] p-1.5" data-testid="sources-try-tree">
      {nodes.map((n) => {
        const p = n.pointer === '' ? '/' : n.pointer
        return (
          <button
            key={n.pointer}
            type="button"
            title="Copy this pointer"
            className="block w-full text-left hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            style={{ paddingLeft: `${Math.min(depth(n.pointer), 10) * 10}px` }}
            onClick={() => {
              void navigator.clipboard?.writeText(n.pointer)
              setCopied(n.pointer)
              setTimeout(() => setCopied((c) => (c === n.pointer ? null : c)), 1500)
            }}
            data-testid="sources-try-node"
          >
            <span className="text-[var(--color-text-primary)]">{p}</span>
            <span className={muted}> · {n.type}</span>
            {n.len !== undefined && <span className={muted}> · {n.len}</span>}
            {n.sample !== undefined && <span className="text-[var(--color-text-secondary)]"> · {sampleText(n.sample)}</span>}
            {copied === n.pointer && <span className={muted}> · copied</span>}
          </button>
        )
      })}
      {truncated && <p className={muted}>… the tree stops at 500 nodes.</p>}
    </div>
  )
}

/** The head line of a Try result: ok/failed, status, time, warnings. */
export function TrySummary({ r }: { r: SourceTryResponse }): React.JSX.Element {
  return (
    <div className="space-y-1 text-[11px]" data-testid="sources-try-summary">
      <p className="text-[var(--color-text-primary)]">
        {r.ok ? 'Answered' : `Failed: ${r.view?.reasonText ?? r.reason ?? 'error'}`}
        {r.status != null && ` · HTTP ${r.status}`} · {r.elapsedMs} ms{r.truncated && ' · too large, not read'}
      </p>
      {(r.targets ?? []).map((t) => (
        <p key={t.key} className={muted}>
          {t.key}: {t.status ?? '—'} {t.reason ?? ''} · {t.elapsedMs} ms
        </p>
      ))}
      {r.warnings.map((w) => (
        <p key={w.code + w.field} className="text-[var(--color-status-warn)]">
          {w.hint}
        </p>
      ))}
      {r.view && (
        <p className={muted}>
          This would read: {r.view.display}
          {r.view.items.length > 0 && ` · ${r.view.items.length} item${r.view.items.length === 1 ? '' : 's'}`}
        </p>
      )}
    </div>
  )
}
