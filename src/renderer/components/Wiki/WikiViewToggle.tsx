// Wiki header view switch: Wiki | 2D | 3D (prd-wiki-3d-view-v1 §3.1, §12).
// Same look as the toolbar's Global | Local control. The header is a window
// drag region, so the wrapper is `no-drag`.

import React from 'react'
import type { WikiView } from '@/lib/wiki-view-pref'

const LABELS: Record<WikiView, { label: string; title: string }> = {
  wiki: { label: 'Wiki', title: 'Read notes (standard wiki)' },
  '2d': { label: '2D', title: 'Flat map (2D)' },
  '3d': { label: '3D', title: '3D map. Drag to orbit' },
}

export function WikiViewToggle({
  value,
  onChange,
  views = ['wiki', '2d', '3d'],
  disabled3dTitle = null,
}: {
  value: WikiView
  onChange: (view: WikiView) => void
  views?: readonly WikiView[]
  /** When set, the 3D button is disabled and this is its title (no WebGL 2). */
  disabled3dTitle?: string | null
}): React.JSX.Element {
  return (
    <div
      role="group"
      aria-label="Wiki view"
      data-wiki-view-toggle
      className="no-drag flex items-center gap-0.5 border border-[var(--color-border)]"
    >
      {views.map((v) => {
        const disabled = v === '3d' && !!disabled3dTitle
        const on = value === v
        return (
          <button
            key={v}
            type="button"
            aria-pressed={on}
            disabled={disabled}
            data-wiki-view={v}
            onClick={() => {
              if (!on) onChange(v)
            }}
            title={disabled ? (disabled3dTitle ?? undefined) : LABELS[v].title}
            className={`px-2 py-1 text-[10px] font-medium transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed ${
              on
                ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {LABELS[v].label}
          </button>
        )
      })}
    </div>
  )
}
