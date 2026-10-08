// Rosson 2026-10-08 — the New Garden modal: a name field and a browsable
// list of the catalog defaults as cards with a small preview sketch (the
// Diary first, then Start with the default and Start empty and ask my
// agent). Pick one, name it, Create: one step.
//
// No permissions (Rosson 2026-10-08): a catalog Garden's widgets work the
// moment it's created, like any widget in your own Garden. Nothing to
// agree to, no scope picker, no agent chooser.
//
// Shared by Zen (the Garden switcher's "+ New Garden", through
// `ZenNewGardenHost`) and Settings → Gardens (its "+ New Garden" and the
// Garden catalog's Add). The caller does the create.

import { useEffect, useMemo, useRef, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenTemplateInfo } from '@/lib/zen/zen-custom-types'
import { loadZenTemplates, useZenCatalogBadges, useZenTemplatesStore, zenCatalogGardenName } from '@/lib/zen/zen-templates'
import { closeZenNewGarden, openZenNewGarden, useZenNewGardenModal, zenNewGardenCards } from '@/lib/zen/zen-new-garden'
import { useZenOpenNewGardenRequest, zenNewGardenNameProblem } from './ZenNewGarden'
import { ZenOverlay, zenButtonStyle } from './ZenOverlay'

// ── Preview sketches ──────────────────────────────────────────────────────

function hueOf(short: string): number {
  let hash = 0
  for (const ch of short) hash = (hash * 31 + ch.charCodeAt(0)) >>> 0
  return hash % 360
}

/** A small drawing of a Garden: K2 draws one per known template, and a
 *  plain page sketch in the entry's own hue for any other. */
export function ZenGardenSketch({ entry }: { entry: ZenTemplateInfo }): React.JSX.Element {
  const common = { width: '100%', height: '100%', viewBox: '0 0 160 90', preserveAspectRatio: 'xMidYMid slice' } as const
  let art: React.JSX.Element
  if (entry.short === 'diary') {
    art = (
      <svg {...common}>
        <rect width="160" height="90" fill="#120c0a" />
        <ellipse cx="34" cy="10" rx="60" ry="30" fill="rgba(255,150,70,0.18)" />
        <path d="M22 10 h112 a4 4 0 0 1 4 4 v62 l-12 8 h-104 z" fill="#e1cfa5" />
        <path d="M22 10 v74" stroke="#8c6f43" strokeWidth="4" />
        <path d="M126 84 l12 -8 h-12 z" fill="#c9b183" />
        <text x="34" y="30" fontFamily="Caveat, cursive" fontSize="15" fontWeight="700" fill="#5c0f0f">
          Cortana
        </text>
        {[44, 54, 64].map((y, i) => (
          <path
            key={y}
            d={`M34 ${y} q10 -3 20 0 t20 0 t20 0 ${i === 1 ? '' : 't20 0'}`}
            fill="none"
            stroke={i === 1 ? 'rgba(28,36,64,0.35)' : '#2a120c'}
            strokeWidth="1.4"
            strokeLinecap="round"
          />
        ))}
        <circle cx="132" cy="80" r="2" fill="#c4471f" />
      </svg>
    )
  } else if (entry.short === 'texting') {
    art = (
      <svg {...common}>
        <rect width="160" height="90" fill="var(--zen-surface, #f3f3f1)" />
        <rect x="8" y="8" width="44" height="74" rx="5" fill="var(--zen-surface-raised, #fff)" stroke="var(--zen-border, #ddd)" />
        {[16, 30, 44, 58].map((y) => (
          <g key={y}>
            <circle cx="17" cy={y + 4} r="4" fill="var(--zen-accent, #6b8afd)" opacity="0.6" />
            <rect x="25" y={y + 1} width="20" height="5" rx="2" fill="var(--zen-text-muted, #999)" opacity="0.5" />
          </g>
        ))}
        <rect x="58" y="8" width="94" height="74" rx="5" fill="var(--zen-surface-raised, #fff)" stroke="var(--zen-border, #ddd)" />
        <rect x="66" y="16" width="48" height="12" rx="6" fill="var(--zen-text-muted, #999)" opacity="0.3" />
        <rect x="96" y="34" width="48" height="12" rx="6" fill="var(--zen-accent, #6b8afd)" opacity="0.7" />
        <rect x="66" y="52" width="58" height="12" rx="6" fill="var(--zen-text-muted, #999)" opacity="0.3" />
      </svg>
    )
  } else if (entry.short === 'blank') {
    art = (
      <svg {...common}>
        <rect width="160" height="90" fill="var(--zen-surface, #f3f3f1)" />
        <rect x="10" y="10" width="140" height="70" rx="6" fill="none" stroke="var(--zen-text-muted, #999)" strokeDasharray="4 4" opacity="0.6" />
        <path d="M80 34 v22 M69 45 h22" stroke="var(--zen-accent, #6b8afd)" strokeWidth="2.5" strokeLinecap="round" />
      </svg>
    )
  } else {
    const hue = hueOf(entry.short)
    art = (
      <svg {...common}>
        <rect width="160" height="90" fill={`hsl(${hue} 45% 88%)`} />
        <rect x="10" y="10" width="44" height="70" rx="4" fill="rgba(255,255,255,0.6)" />
        <rect x="62" y="10" width="88" height="14" rx="3" fill={`hsl(${hue} 35% 35% / 0.5)`} />
        <rect x="62" y="30" width="88" height="50" rx="4" fill="rgba(255,255,255,0.7)" />
      </svg>
    )
  }
  return (
    <div
      aria-hidden
      data-zen-garden-sketch={entry.short}
      style={{
        width: '100%',
        aspectRatio: '16 / 9',
        overflow: 'hidden',
        borderRadius: 'calc(var(--zen-radius, 8px) - 4px)',
        border: '1px solid var(--zen-border)',
      }}
    >
      {art}
    </div>
  )
}

// ── The modal ─────────────────────────────────────────────────────────────

export interface ZenNewGardenPick {
  /** The new Garden's name (checked). */
  name: string
  entry: ZenTemplateInfo
  /** Start empty: open Ask my agent once it shows. */
  ask: boolean
}

export function ZenNewGardenModal({
  taken,
  highlight = null,
  onCreate,
  onClose,
  vars,
}: {
  /** Names already used (a clash is refused before the create). */
  taken: readonly string[]
  /** A template short to select first ("See it in New Garden", Settings' Add). */
  highlight?: string | null
  onCreate(pick: ZenNewGardenPick): Promise<void>
  onClose(): void
  vars?: React.CSSProperties
}): React.JSX.Element {
  const templates = useZenTemplatesStore((s) => s.templates)
  const badges = useZenCatalogBadges((s) => s.badges)
  const cards = useMemo(() => zenNewGardenCards(templates), [templates])
  const [picked, setPicked] = useState<string | null>(highlight)
  const selected = cards.find((t) => t.short === picked) ?? cards[0] ?? null
  const [name, setName] = useState('')
  const [touched, setTouched] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nameRef = useRef<HTMLInputElement | null>(null)

  useEffect(() => {
    // The catalog may have grown since the last time (UWB23).
    void loadZenTemplates()
  }, [])
  useEffect(() => {
    if (highlight) setPicked(highlight)
  }, [highlight])
  // Until the person types a name, it follows the pick ("Diary", "Garden 4").
  const suggested =
    selected && selected.section === 'catalog' ? zenCatalogGardenName(selected.label, taken) : nextGardenName(taken)
  const shown = touched ? name : suggested
  // The name field takes focus (its text selected) as soon as the modal's
  // portal mounts it, before the overlay's own focus runs.
  const focusedOnce = useRef(false)
  const nameField = (el: HTMLInputElement | null): void => {
    nameRef.current = el
    if (!el || focusedOnce.current) return
    focusedOnce.current = true
    el.focus()
    el.select()
  }

  const create = (): void => {
    if (busy || !selected) return
    const problem = zenNewGardenNameProblem(shown, taken)
    if (problem) {
      setError(problem)
      nameRef.current?.focus()
      return
    }
    setBusy(true)
    setError(null)
    void onCreate({ name: shown.trim(), entry: selected, ask: selected.short === 'blank' })
      .then(() => {
        setBusy(false)
        onClose()
      })
      .catch((err: unknown) => {
        setBusy(false)
        setError(err instanceof Error ? err.message : String(err))
      })
  }

  return (
    <ZenOverlay label="New Garden" onClose={onClose} testId="zen-new-garden-modal" vars={vars} width={600}>
      <strong style={{ fontSize: '1.1em' }}>New Garden</strong>
      <label className="flex flex-col" style={{ gap: 4 }}>
        <span style={{ fontSize: '0.85em', color: 'var(--zen-text-muted)' }}>Name</span>
        <input
          ref={nameField}
          type="text"
          value={shown}
          maxLength={60}
          aria-label="New Garden name"
          data-zen-new-garden-name=""
          disabled={busy}
          onChange={(e) => {
            setTouched(true)
            setName(e.target.value)
            if (error) setError(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.nativeEvent.isComposing) {
              e.preventDefault()
              create()
            }
            // Plain keys stay in the field; ⌘ / Ctrl chords reach the app.
            if (!e.metaKey && !e.ctrlKey && e.key !== 'Escape') e.stopPropagation()
          }}
          style={{
            height: 32,
            padding: '0 10px',
            color: 'var(--zen-text)',
            background: 'var(--zen-surface)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'calc(var(--zen-radius) - 4px)',
            font: 'inherit',
            outline: 'none',
          }}
        />
      </label>
      <div
        role="radiogroup"
        aria-label="Start from"
        data-zen-new-garden-cards=""
        style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fill, minmax(160px, 1fr))', gap: 10 }}
        onKeyDown={(e) => {
          if (!['ArrowRight', 'ArrowDown', 'ArrowLeft', 'ArrowUp'].includes(e.key) || cards.length === 0) return
          e.preventDefault()
          const at = Math.max(0, cards.findIndex((t) => t.short === selected?.short))
          const step = e.key === 'ArrowRight' || e.key === 'ArrowDown' ? 1 : -1
          const to = cards[(at + step + cards.length) % cards.length]
          setPicked(to.short)
          e.currentTarget.querySelector<HTMLElement>(`[data-zen-new-garden-card="${to.short}"]`)?.focus()
        }}
      >
        {cards.map((t) => {
          const on = t.short === selected?.short
          return (
            <button
              key={t.short}
              type="button"
              role="radio"
              aria-checked={on}
              tabIndex={on ? 0 : -1}
              disabled={busy}
              data-zen-new-garden-card={t.short}
              data-zen-new-garden-section={t.section}
              onClick={() => setPicked(t.short)}
              className="flex flex-col items-stretch text-left cursor-pointer disabled:cursor-default"
              style={{
                gap: 6,
                padding: 8,
                color: 'var(--zen-text)',
                background: on ? 'var(--zen-surface)' : 'transparent',
                border: `1px solid ${on ? 'var(--zen-accent)' : 'var(--zen-border)'}`,
                borderRadius: 'var(--zen-radius)',
                boxShadow: on ? '0 0 0 1px var(--zen-accent)' : 'none',
              }}
            >
              <ZenGardenSketch entry={t} />
              <span className="flex items-center" style={{ gap: 6, fontWeight: 600 }}>
                <span className="truncate">{t.label}</span>
                {badges[t.short] && (
                  <span
                    data-zen-catalog-badge={t.short}
                    style={{
                      fontSize: '0.65em',
                      padding: '0 5px',
                      border: '1px solid var(--zen-accent)',
                      borderRadius: 999,
                      color: 'var(--zen-accent)',
                      textTransform: 'uppercase',
                      letterSpacing: '0.04em',
                    }}
                  >
                    {badges[t.short]}
                  </span>
                )}
              </span>
              <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>{t.description}</span>
            </button>
          )
        })}
      </div>
      {error && (
        <span role="alert" data-zen-new-garden-error="" style={{ fontSize: '0.85em', color: 'var(--zen-danger)' }}>
          {error}
        </span>
      )}
      <div className="flex items-center justify-end" style={{ gap: 8 }}>
        <button type="button" data-zen-new-garden-cancel="" disabled={busy} onClick={onClose} className="cursor-pointer" style={zenButtonStyle(false)}>
          Cancel
        </button>
        <button
          type="button"
          data-zen-new-garden-create=""
          disabled={busy || !selected}
          onClick={create}
          className="cursor-pointer disabled:cursor-default"
          style={{ ...zenButtonStyle(true), opacity: busy || !selected ? 0.5 : 1 }}
        >
          {busy ? 'Creating…' : 'Create'}
        </button>
      </div>
    </ZenOverlay>
  )
}

/** "Garden N" for a plain start, the first free number. */
function nextGardenName(taken: readonly string[]): string {
  const used = new Set(taken.map((n) => n.toLocaleLowerCase()))
  for (let n = taken.length + 1; ; n++) {
    const name = `Garden ${n}`
    if (!used.has(name.toLocaleLowerCase())) return name
  }
}

/** Zen's New Garden: one per page (`ZenPage`), drawn while the modal is
 *  open; the create goes through the bridge (`gardens.create`, cap
 *  `gardens:manage`), which also switches this window to the new Garden.
 *  It also answers "See it in New Garden" (`k2:zen-open-new-garden`). */
export function ZenNewGardenHost({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element | null {
  const open = useZenNewGardenModal((s) => s.open)
  const highlight = useZenNewGardenModal((s) => s.highlight)
  const manage = bridge.caps.has('gardens:manage')
  useZenOpenNewGardenRequest(manage, (short) => openZenNewGarden(short))
  // Leaving Zen (or this page going away) closes it.
  useEffect(() => () => closeZenNewGarden(), [])
  if (!open || !manage) return null
  return (
    <ZenNewGardenModal
      taken={bridge.gardens.list().map((g) => g.name)}
      highlight={highlight}
      onClose={closeZenNewGarden}
      onCreate={async (pick) => {
        await bridge.gardens.create(pick.name, pick.entry.short, { ask: pick.ask })
      }}
    />
  )
}
