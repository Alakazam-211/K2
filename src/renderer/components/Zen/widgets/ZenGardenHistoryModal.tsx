// prd-garden-access-v1 G26a (rev 5, Rosson: "part of each garden's and
// widget's audit list that isn't super in the way but can be opened up for
// either in a modal window"). The two history modals, one component:
//
//   - Garden history: from Settings → Gardens → <Garden> → History…. The
//     Garden's events plus the events of the widgets placed in it.
//   - Widget history: from the pane's widget History…, and from the widget's
//     ⋯ menu in Zen (`ZenCustomWidget`'s CornerMenu, outside the frame).
//
// Layout: a title ("Fleet Ops history" / "fleet-edge history"), the filters
// (All · Changes · Access · Asks), newest first, 50 a page with Load more.
// Each line: who (name and colour), what, when. A line with a snapshot has
// View this version and Restore.
//
// "Not signed" (GA1) shows ONLY here, as the top line, with Sign: "This
// version was changed outside K2 and isn't signed, so it runs without your
// data." Sign is the person's one click (`POST zen/widget/sign`). A deleted
// Garden's history is read-only (no Sign, no Restore).
//
// Thin client: the daemon merges events, snapshots, stamps and proposals
// (G29) and decides what is unsigned; this draws its answer. Every call is
// in `lib/zen/garden-access-api.ts`. K2's standard overlay (`ZenOverlay`),
// so it carries the Garden's tokens in Zen and Settings' look in Settings.

import { useCallback, useEffect, useRef, useState } from 'react'
import {
  fetchGardenHistory,
  fetchSnapshotText,
  fetchWidgetHistory,
  GARDEN_HISTORY_FILTERS,
  gardenAccessErrorText,
  gardenHistoryLine,
  gardenWhen,
  restoreGardenVersion,
  signGardenWidget,
  type GardenHistoryFilter,
  type GardenHistoryItem,
  type GardenHistoryPage,
  type GardenUnsigned,
} from '@/lib/zen/garden-access-api'
import { ZenOverlay, zenButtonStyle } from './ZenOverlay'

export type GardenHistoryTarget =
  | { kind: 'garden'; garden: string; name: string; readOnly?: boolean }
  | {
      kind: 'widget'
      widget: string
      /** The Garden it was opened from (Sign signs for it); null = any. */
      garden: string | null
      readOnly?: boolean
    }

export const GARDEN_NOT_SIGNED_TEXT =
  'This version was changed outside K2 and isn’t signed, so it runs without your data.'

/** 0.46.5 S0: a widget's version now covers its manifest, so a version
 *  saved by an older K2 comes back with a new version hash; only the live
 *  one's signature was carried at upgrade. */
export const GARDEN_RESTORE_OLD_VERSION_TEXT =
  'A version saved before K2 0.46.5 comes back as a new version, so it may need signing again.'

const PAGE = 50

function title(t: GardenHistoryTarget): string {
  return t.kind === 'garden' ? `${t.name} history` : `${t.widget} history`
}

const LINK: React.CSSProperties = {
  padding: 0,
  border: 'none',
  background: 'none',
  color: 'var(--zen-accent)',
  cursor: 'pointer',
  font: 'inherit',
  fontSize: '0.85em',
}

function errText(err: unknown): string {
  return gardenAccessErrorText(err)
}

function HistoryLine({
  item,
  target,
  readOnly,
  onRestored,
}: {
  item: GardenHistoryItem
  target: GardenHistoryTarget
  readOnly: boolean
  onRestored(): void
}): React.JSX.Element {
  const [view, setView] = useState<string | null>(null)
  const [confirming, setConfirming] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const snap = item.snapshot
  const widget = item.widget ?? (target.kind === 'widget' ? target.widget : null)
  const restoreTarget = widget ? { widget } : target.kind === 'garden' ? { garden: target.garden } : null

  const toggleView = (): void => {
    if (!snap) return
    if (view !== null) {
      setView(null)
      return
    }
    setBusy(true)
    setError(null)
    fetchSnapshotText(snap)
      .then((t) => setView(t))
      .catch((err: unknown) => setError(errText(err)))
      .finally(() => setBusy(false))
  }

  const restore = (): void => {
    if (!snap || !restoreTarget) return
    setBusy(true)
    setError(null)
    restoreGardenVersion(restoreTarget, snap.name)
      .then(() => {
        setConfirming(false)
        onRestored()
      })
      .catch((err: unknown) => setError(errText(err)))
      .finally(() => setBusy(false))
  }

  const color = item.actor.color
  return (
    <li
      data-garden-history-line={item.id}
      data-garden-history-kind={item.kind}
      data-garden-history-source={item.source}
      style={{ padding: '8px 0', borderBottom: '1px solid var(--zen-border)', listStyle: 'none' }}
    >
      <div className="flex items-start" style={{ gap: 8 }}>
        <span
          aria-hidden="true"
          style={{
            flex: '0 0 auto',
            width: 8,
            height: 8,
            marginTop: 5,
            borderRadius: 999,
            background: color ?? 'var(--zen-text-muted)',
            opacity: color ? 1 : 0.4,
          }}
        />
        <div className="flex-1 min-w-0">
          <div data-garden-history-text="">{gardenHistoryLine(item)}</div>
          <div className="flex items-center flex-wrap" style={{ gap: 10, color: 'var(--zen-text-muted)', fontSize: '0.85em' }}>
            <span data-garden-history-when="">{gardenWhen(item.at)}</span>
            {snap && (
              <button type="button" data-garden-history-view="" style={LINK} disabled={busy} onClick={toggleView}>
                {view !== null ? 'Hide this version' : 'View this version'}
              </button>
            )}
            {snap && restoreTarget && !readOnly && !confirming && (
              <button type="button" data-garden-history-restore="" style={LINK} disabled={busy} onClick={() => setConfirming(true)}>
                Restore
              </button>
            )}
          </div>
          {confirming && (
            <div className="flex items-center flex-wrap" style={{ gap: 8, marginTop: 6 }} data-garden-history-restore-confirm="">
              <span>
                Restore this version? K2 keeps the current one in history first.
                {widget && <> {GARDEN_RESTORE_OLD_VERSION_TEXT}</>}
              </span>
              <button type="button" style={zenButtonStyle(true)} disabled={busy} onClick={restore}>
                Restore
              </button>
              <button type="button" style={zenButtonStyle(false)} disabled={busy} onClick={() => setConfirming(false)}>
                Cancel
              </button>
            </div>
          )}
          {view !== null && (
            <pre
              data-garden-history-version=""
              style={{
                marginTop: 6,
                maxHeight: 240,
                overflow: 'auto',
                padding: 8,
                fontSize: '0.8em',
                whiteSpace: 'pre-wrap',
                background: 'var(--zen-surface)',
                border: '1px solid var(--zen-border)',
                borderRadius: 'calc(var(--zen-radius) - 4px)',
              }}
            >
              {view}
            </pre>
          )}
          {error && (
            <p role="alert" style={{ color: 'var(--zen-danger)', fontSize: '0.85em', marginTop: 4 }}>
              {error}
            </p>
          )}
        </div>
      </div>
    </li>
  )
}

function UnsignedLine({
  u,
  showWidget,
  signFor,
  readOnly,
  onSigned,
}: {
  u: GardenUnsigned
  showWidget: boolean
  signFor: string | null
  readOnly: boolean
  onSigned(): void
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const garden = signFor ?? u.gardens[0] ?? null
  return (
    <div
      role="status"
      data-garden-not-signed={u.widget}
      style={{
        padding: 10,
        border: '1px solid var(--zen-border)',
        borderRadius: 'calc(var(--zen-radius) - 4px)',
        background: 'var(--zen-surface)',
      }}
    >
      <div className="flex items-start" style={{ gap: 10 }}>
        <span className="flex-1">
          {showWidget ? <strong>{u.widget}: </strong> : null}
          {GARDEN_NOT_SIGNED_TEXT}
        </span>
        {!readOnly && garden && (
          <button
            type="button"
            data-garden-sign={u.widget}
            style={zenButtonStyle(true)}
            disabled={busy}
            onClick={() => {
              setBusy(true)
              setError(null)
              signGardenWidget(u.widget, garden)
                .then(onSigned)
                .catch((err: unknown) => setError(errText(err)))
                .finally(() => setBusy(false))
            }}
          >
            Sign
          </button>
        )}
      </div>
      {error && (
        <p role="alert" style={{ color: 'var(--zen-danger)', fontSize: '0.85em', marginTop: 4 }}>
          {error}
        </p>
      )}
    </div>
  )
}

export function ZenGardenHistoryModal({
  target,
  vars,
  onClose,
}: {
  target: GardenHistoryTarget
  /** Settings maps `--zen-*` onto its own tokens here. */
  vars?: React.CSSProperties
  onClose(): void
}): React.JSX.Element {
  const [filter, setFilter] = useState<GardenHistoryFilter>('all')
  const [items, setItems] = useState<GardenHistoryItem[]>([])
  const [unsigned, setUnsigned] = useState<GardenUnsigned[]>([])
  const [next, setNext] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const seq = useRef(0)
  const readOnly = target.readOnly === true
  const key = target.kind === 'garden' ? `g:${target.garden}` : `w:${target.widget}`

  const fetchPage = useCallback(
    (before: string | null, f: GardenHistoryFilter): Promise<GardenHistoryPage> =>
      target.kind === 'garden'
        ? fetchGardenHistory(target.garden, { before, filter: f, limit: PAGE })
        : fetchWidgetHistory(target.widget, { before, filter: f, limit: PAGE }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key],
  )

  /** Page one for the filter (the unsigned lines come with it). */
  const reload = useCallback(
    (f: GardenHistoryFilter) => {
      const mine = ++seq.current
      setLoading(true)
      setError(null)
      fetchPage(null, f)
        .then((p) => {
          if (mine !== seq.current) return
          setItems(p.items)
          setNext(p.next)
          setUnsigned(p.unsigned)
        })
        .catch((err: unknown) => {
          if (mine === seq.current) setError(errText(err))
        })
        .finally(() => {
          if (mine === seq.current) setLoading(false)
        })
    },
    [fetchPage],
  )

  useEffect(() => reload(filter), [reload, filter])

  const loadMore = (): void => {
    if (!next) return
    const mine = ++seq.current
    setLoading(true)
    fetchPage(next, filter)
      .then((p) => {
        if (mine !== seq.current) return
        setItems((prev) => [...prev, ...p.items])
        setNext(p.next)
      })
      .catch((err: unknown) => {
        if (mine === seq.current) setError(errText(err))
      })
      .finally(() => {
        if (mine === seq.current) setLoading(false)
      })
  }

  // Sign signs for the Garden the modal belongs to (`'*'` has no widgets).
  const signFor = target.garden === '*' ? null : target.garden
  return (
    <ZenOverlay label={title(target)} onClose={onClose} testId="zen-garden-history-modal" vars={vars} width={620}>
      <div className="flex items-center" style={{ gap: 8 }}>
        <strong style={{ fontSize: '1.1em' }} className="flex-1 truncate" data-garden-history-title="">
          {title(target)}
        </strong>
        <button type="button" aria-label="Close" style={zenButtonStyle(false)} onClick={onClose}>
          Close
        </button>
      </div>
      {unsigned.map((u) => (
        <UnsignedLine
          key={u.widget}
          u={u}
          showWidget={target.kind === 'garden'}
          signFor={signFor}
          readOnly={readOnly}
          onSigned={() => reload(filter)}
        />
      ))}
      <div role="group" aria-label="Show" className="flex flex-wrap" style={{ gap: 6 }} data-garden-history-filters="">
        {GARDEN_HISTORY_FILTERS.map((f) => (
          <button
            key={f.id}
            type="button"
            aria-pressed={filter === f.id}
            data-garden-history-filter={f.id}
            onClick={() => setFilter(f.id)}
            style={{
              ...zenButtonStyle(false),
              padding: '3px 10px',
              borderColor: filter === f.id ? 'var(--zen-accent)' : 'var(--zen-border)',
              color: filter === f.id ? 'var(--zen-accent)' : 'var(--zen-text)',
            }}
          >
            {f.label}
          </button>
        ))}
      </div>
      {error && (
        <p role="alert" data-garden-history-error="" style={{ color: 'var(--zen-danger)', fontSize: '0.9em' }}>
          {error}
        </p>
      )}
      {!loading && !error && items.length === 0 && (
        <p data-garden-history-empty="" style={{ color: 'var(--zen-text-muted)' }}>
          Nothing here yet.
        </p>
      )}
      <ul data-garden-history-list="" style={{ margin: 0, padding: 0 }}>
        {items.map((it) => (
          <HistoryLine key={it.id} item={it} target={target} readOnly={readOnly} onRestored={() => reload(filter)} />
        ))}
      </ul>
      {loading && <p style={{ color: 'var(--zen-text-muted)' }}>Loading…</p>}
      {next && !loading && (
        <button type="button" data-garden-history-more="" style={zenButtonStyle(false)} onClick={loadMore}>
          Load more
        </button>
      )}
    </ZenOverlay>
  )
}
