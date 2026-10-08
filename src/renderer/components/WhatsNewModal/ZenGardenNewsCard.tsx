// prd-zen-garden-sync-defaults-v1 GS40–GS43 — the Garden news card on the
// LEFT of the "What's new in K2" dialog, mirroring "Enjoying K2?" on its
// right (`GithubStarDrawer`): same Surface, same type sizes and button, the
// ensō where the logo is, tucked behind the dialog.
//
// It shows one item (newest first), "+ N more in Settings → Gardens" when
// there are more, and the copies line when some own copies have a newer
// default. News comes from THIS computer's daemon (`zenLocalScope`), never
// the window's server. It has no × of its own: it leaves with the dialog,
// and closing the dialog marks what it showed as seen (the dialog calls
// `onShown` with those ids). A click on its button also marks them seen,
// closes the dialog and does the action. Hidden below 1000 px (and then
// nothing is marked seen; Settings → Gardens keeps the items).

import { useEffect, useState } from 'react'
import { Surface } from '@/components/ui'
import ZenIcon from '@/components/Zen/ZenIcon'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { zenAvailable } from '@/lib/zen/zen-platform'
import { useSettingsStore } from '@/stores/settings'
import { useToastStore } from '@/stores/toast'
import {
  keepZenPreviousLook,
  loadZenNews,
  markZenNewsSeen,
  undoZenGardenSync,
  useZenSyncStore,
  zenNewsCardModel,
  zenNewsCardText,
  type ZenNewsItem,
} from '@/lib/zen/zen-sync'

/** Fired when "See it in New Garden" asks Zen's New Garden to open with
 *  this catalog entry highlighted (`detail: {short}`). */
export const ZEN_OPEN_NEW_GARDEN_EVENT = 'k2:zen-open-new-garden'

function useViewportWidth(): number {
  const [w, setW] = useState(() => (typeof window === 'undefined' ? 0 : window.innerWidth))
  useEffect(() => {
    const on = (): void => setW(window.innerWidth)
    window.addEventListener('resize', on)
    return () => window.removeEventListener('resize', on)
  }, [])
  return w
}

/** Do the card's action for `item` (GS43). */
export async function runZenNewsAction(item: ZenNewsItem): Promise<void> {
  if (item.kind === 'catalog') {
    if (useZenWindowStore.getState().on) {
      window.dispatchEvent(new CustomEvent(ZEN_OPEN_NEW_GARDEN_EVENT, { detail: { short: item.short } }))
    } else {
      useSettingsStore.getState().openSettings('zen-gardens')
    }
    return
  }
  await keepZenPreviousLook(item.garden, item.part)
  useToastStore.getState().addToast(`${item.gardenName} keeps its previous look`, 'success', 8000, {
    label: 'Undo',
    onClick: () => void undoZenGardenSync(item.garden),
  })
}

export function ZenGardenNewsCard({
  dialogVisible,
  onShown,
  onAction,
}: {
  dialogVisible: boolean
  /** The ids the card is showing now (empty when hidden). */
  onShown(ids: string[]): void
  /** Close the dialog after the card's button ran. */
  onAction(): void
}): React.JSX.Element | null {
  const news = useZenSyncStore((s) => s.news)
  const width = useViewportWidth()
  const [busy, setBusy] = useState(false)

  // Zen is this computer's (the desktop app only): no card elsewhere.
  const local = zenAvailable()
  useEffect(() => {
    if (!dialogVisible || !local) return
    void loadZenNews().catch((err: unknown) => console.debug('[zen-sync] news:', err))
  }, [dialogVisible, local])

  const model = local ? zenNewsCardModel(news, { dialogVisible, viewportWidth: width }) : null
  const shown = model ? model.shownIds.join('\n') : ''
  useEffect(() => {
    onShown(shown === '' ? [] : shown.split('\n'))
  }, [shown, onShown])

  if (!model) return null
  const text = zenNewsCardText(model.item)
  const act = (e: React.MouseEvent): void => {
    e.stopPropagation()
    if (busy) return
    setBusy(true)
    void markZenNewsSeen(model.shownIds)
      .then(() => runZenNewsAction(model.item))
      .catch((err: unknown) => console.warn('[zen-sync] card action:', err))
      .finally(() => {
        setBusy(false)
        onAction()
      })
  }
  const openSettings = (e: React.MouseEvent): void => {
    e.stopPropagation()
    void markZenNewsSeen(model.shownIds).finally(() => {
      useSettingsStore.getState().openSettings('zen-gardens')
      onAction()
    })
  }

  return (
    <Surface
      role2="inset"
      bordered
      className="no-drag"
      data-zen-news-card=""
      data-zen-news-item={model.item.id}
      style={{
        position: 'absolute',
        right: '100%',
        top: '22%',
        zIndex: 1,
        width: 176,
        marginRight: -10,
        padding: '16px 22px 14px 14px',
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-start',
        gap: 10,
        boxShadow: '-4px 8px 20px rgba(0, 0, 0, 0.28)',
        fontFamily: "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Inter', system-ui, sans-serif",
      }}
    >
      <span style={{ color: 'var(--color-accent, #4a9eff)' }}>
        <ZenIcon variant="enso" on size={28} accent="var(--color-accent, #4a9eff)" />
      </span>
      <div style={{ fontSize: '14px', fontWeight: 600, color: 'var(--color-text-primary)', lineHeight: 1.3 }}>{text.title}</div>
      <div style={{ fontSize: '12.5px', lineHeight: 1.5, color: 'var(--color-text-secondary)' }}>
        {text.lead && <strong style={{ fontWeight: 600, color: 'var(--color-text-primary)' }}>{text.lead} </strong>}
        {text.body}
      </div>
      {model.copiesLine && (
        <div data-zen-news-copies="" style={{ fontSize: '12px', lineHeight: 1.45, color: 'var(--color-text-secondary)' }}>
          Your own copies didn’t change. Sync them in Settings → Gardens.
        </div>
      )}
      {text.button && (
        <button type="button" className="wn-star-btn" disabled={busy} onClick={act} data-zen-news-action="">
          {text.button}
        </button>
      )}
      {(model.item.kind === 'update' || model.more > 0) && (
        <button
          type="button"
          onClick={openSettings}
          data-zen-news-settings=""
          style={{ fontSize: '12px', color: 'var(--color-accent, #4a9eff)', background: 'none', border: 0, padding: 0, cursor: 'pointer' }}
        >
          {model.more > 0 ? `+ ${model.more} more in Settings → Gardens` : 'Settings → Gardens'}
        </button>
      )}
    </Surface>
  )
}
