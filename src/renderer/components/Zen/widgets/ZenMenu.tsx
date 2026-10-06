// prd-zen-freeform-chrome FC15–FC19, FC25 — `kind = "menu"`: a K2-drawn
// button in a band or column edge that opens a list of K2's controls.
//
// The button (FC15): `icon` dots (⋯, default), bars (☰) or zen (the ensō),
// plus the `label` when the file gives one; the label (else "More") is the
// tooltip and the screen-reader name. At least 30×30, a shared Zen glass
// tile. It is bound as `zen-menu` with its menu id: K2 attaches no action,
// never cancels its keys, and checks that what it holds turns up within
// 1 s of an activation (FC25).
//
// What it holds (FC16, FC17), in file order, with separators between:
//   - `garden-switcher`: a Gardens section listing every Garden (each a
//     bound `garden-option` with its ⌥⌘N hint), then + New Garden. Inline,
//     never a sub-menu, so the switcher stays one click away (Q5);
//   - `zen-toggle`: Exit Zen Mode ⌃⌘Z, bound as `zen-toggle`;
//   - `theme-picker`: Theme: <name> ›, which swaps to the theme list (Back
//     at the top);
//   - `usage`: Usage ›, which swaps to the usage panel (Back at the top).
//
// How it opens (FC18): click, or Enter, Space or ↓ on the focused button;
// never on hover. Toward the page (down from the top band or a top edge,
// up from the bottom band or a bottom edge), lined up with the button's
// side, flipping when there's no room (`useAnchoredMenu`, portalled into
// the Zen root). While open it is a registered K2 overlay. It closes on a
// pick, Esc, Tab or a click outside.
//
// Keyboard (FC19): opening focuses the first item; ↑/↓ move and wrap;
// Home/End jump; Enter/Space pick; ← or Esc in a sub-panel goes back; Esc
// at the top closes and puts focus back on the button; Tab closes and
// moves on. Disabled items are skipped.

import { useCallback, useContext, useEffect, useRef, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { ZEN_GLASS_PROPS } from '@/lib/zen/zen-glass'
import { zenMenuLabel, zenThemeLabel, type ZenChromeItem } from '@/lib/zen/zen-page'
import { currentDesktopOs } from '@/lib/zen/zen-platform'
import { ZEN_CHORD_LABEL } from '@/lib/zen/zen-shortcut'
import { ZenUsageTool } from '../ZenUsageTool'
import { useZenBind, useZenK2Overlay, ZenChromePlaceContext, ZenK2ChromeContext } from '../ZenTemplateControls'
import { GardenChoice, ZEN_DROPDOWN_STYLE } from './ZenChromeControls'
import { ZenNewGarden } from './ZenNewGarden'

/** A menu button's icons (FC15). */
export const ZEN_MENU_ICONS = ['dots', 'bars', 'zen'] as const
export type ZenMenuIcon = (typeof ZEN_MENU_ICONS)[number]

function menuIcon(item: ZenChromeItem): ZenMenuIcon {
  const icon = item.props.icon
  return typeof icon === 'string' && (ZEN_MENU_ICONS as readonly string[]).includes(icon) ? (icon as ZenMenuIcon) : 'dots'
}

function MenuGlyph({ icon }: { icon: ZenMenuIcon }): React.JSX.Element {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" aria-hidden data-zen-menu-icon={icon} style={{ flexShrink: 0 }}>
      {icon === 'dots' ? (
        <g fill="currentColor">
          <circle cx="3.5" cy="8" r="1.4" />
          <circle cx="8" cy="8" r="1.4" />
          <circle cx="12.5" cy="8" r="1.4" />
        </g>
      ) : icon === 'bars' ? (
        <g stroke="currentColor" strokeWidth="1.5" strokeLinecap="round">
          <path d="M3 4.5h10M3 8h10M3 11.5h10" />
        </g>
      ) : (
        <path
          d="M12.6 5.2A5.2 5.2 0 1 0 13 9.6"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
        />
      )}
    </svg>
  )
}

const ROW_STYLE: React.CSSProperties = {
  minHeight: 32,
  padding: '6px 10px',
  borderRadius: 'calc(var(--zen-radius) - 4px)',
  color: 'var(--zen-text)',
}

const ITEM_SELECTOR = '[role="menuitem"], [role="menuitemradio"]'

function MenuSeparator(): React.JSX.Element {
  return <div role="separator" aria-hidden style={{ height: 1, margin: '3px 6px', background: 'var(--zen-border)' }} />
}

function SectionTitle({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div
      role="presentation"
      data-zen-menu-section-title=""
      style={{
        padding: '4px 10px 2px',
        fontSize: '0.75em',
        fontWeight: 600,
        letterSpacing: '0.06em',
        textTransform: 'uppercase',
        color: 'var(--zen-text-muted)',
      }}
    >
      {children}
    </div>
  )
}

/** Exit Zen Mode, bound as the Zen toggle (FC17). */
function ExitRow({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const ref = useZenBind(bridge, 'zen-toggle')
  const chord = ZEN_CHORD_LABEL[currentDesktopOs() === 'mac' ? 'mac' : 'other']
  return (
    <button
      ref={ref}
      type="button"
      role="menuitem"
      data-zen-menu-exit=""
      className="flex w-full items-center gap-3 text-left cursor-pointer"
      style={ROW_STYLE}
    >
      <span className="min-w-0 flex-1">Exit Zen Mode</span>
      <kbd style={{ color: 'var(--zen-text-muted)', fontSize: '0.8em', fontFamily: 'inherit' }}>{chord}</kbd>
    </button>
  )
}

/** A row that swaps the menu to a sub-panel (Theme ›, Usage ›). */
function PanelRow({
  panel,
  onOpen,
  children,
}: {
  panel: string
  onOpen(): void
  children: React.ReactNode
}): React.JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      aria-haspopup="menu"
      data-zen-menu-panel-row={panel}
      onClick={onOpen}
      className="flex w-full items-center gap-3 text-left cursor-pointer"
      style={ROW_STYLE}
    >
      <span className="min-w-0 flex-1 truncate">{children}</span>
      <span aria-hidden style={{ color: 'var(--zen-text-muted)' }}>
        ›
      </span>
    </button>
  )
}

function BackRow({ onBack }: { onBack(): void }): React.JSX.Element {
  return (
    <button
      type="button"
      role="menuitem"
      data-zen-menu-back=""
      onClick={onBack}
      className="flex w-full items-center gap-3 text-left cursor-pointer"
      style={{ ...ROW_STYLE, color: 'var(--zen-text-muted)' }}
    >
      <span aria-hidden>‹</span>
      <span>Back</span>
    </button>
  )
}

type Panel = 'top' | 'theme' | 'usage'

export function ZenMenu({
  item,
  bridge,
  menuItems,
}: {
  item: ZenChromeItem
  bridge: ZenWidgetBridge
  /** What it holds, in file order (`page.menus[item.id]`). */
  menuItems: readonly ZenChromeItem[]
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const [panel, setPanel] = useState<Panel>('top')
  const place = useContext(ZenChromePlaceContext)
  const k2 = useContext(ZenK2ChromeContext)
  const label = zenMenuLabel(item)
  const shownLabel = typeof item.props.label === 'string' && item.props.label.trim() ? item.props.label.trim() : null
  const bind = useZenBind(bridge, 'zen-menu', item.id)
  const buttonEl = useRef<HTMLButtonElement | null>(null)
  const listEl = useRef<HTMLDivElement | null>(null)
  // Where focus goes when a sub-panel steps back: its opener row.
  const backTo = useRef<Panel | null>(null)
  const buttonRef = useCallback(
    (el: HTMLButtonElement | null) => {
      buttonEl.current = el
      return bind(el)
    },
    [bind],
  )
  const close = useCallback(() => {
    setOpen(false)
    setPanel('top')
  }, [])
  const closeToButton = useCallback(() => {
    close()
    buttonEl.current?.focus()
  }, [close])
  const menu = useAnchoredMenu<HTMLDivElement>({
    open,
    onClose: close,
    gap: 6,
    width: 'min',
    minWidth: 240,
    prefer: place.opens,
    align: place.align === 'end' ? 'end' : 'start',
  })
  const overlay = useZenK2Overlay()
  const listRef = useCallback(
    (el: HTMLDivElement | null) => {
      listEl.current = el
      menu.menuRef(el)
      overlay(el)
    },
    [menu.menuRef, overlay],
  )

  const items = (): HTMLElement[] =>
    Array.from(listEl.current?.querySelectorAll<HTMLElement>(ITEM_SELECTOR) ?? []).filter(
      (el) => !(el as HTMLButtonElement).disabled && el.getAttribute('aria-disabled') !== 'true',
    )

  // Opening (and every panel swap) focuses the first item, or the row a
  // sub-panel came from when it steps back.
  useEffect(() => {
    if (!open) return
    const from = backTo.current
    backTo.current = null
    const target = from
      ? listEl.current?.querySelector<HTMLElement>(`[data-zen-menu-panel-row="${from}"]`)
      : items()[0]
    target?.focus()
  }, [open, panel])

  const openPanel = (next: Panel): void => setPanel(next)
  const back = (): void => {
    backTo.current = panel
    setPanel('top')
  }

  const onListKey = (e: React.KeyboardEvent<HTMLDivElement>): void => {
    const list = items()
    const at = list.indexOf(document.activeElement as HTMLElement)
    const move = (to: number): void => {
      e.preventDefault()
      if (list.length === 0) return
      list[(to + list.length) % list.length].focus()
    }
    switch (e.key) {
      case 'ArrowDown':
        move(at < 0 ? 0 : at + 1)
        return
      case 'ArrowUp':
        move(at < 0 ? list.length - 1 : at - 1)
        return
      case 'Home':
        move(0)
        return
      case 'End':
        move(list.length - 1)
        return
      case 'ArrowLeft':
        if (panel !== 'top') {
          e.preventDefault()
          back()
        }
        return
      case 'ArrowRight': {
        const row = (document.activeElement as HTMLElement | null)?.getAttribute('data-zen-menu-panel-row')
        if (row) {
          e.preventDefault()
          ;(document.activeElement as HTMLElement).click()
        }
        return
      }
      case 'Escape':
        e.preventDefault()
        e.stopPropagation()
        if (panel !== 'top') back()
        else closeToButton()
        return
      case 'Tab':
        // Closes and moves on from the button (the key's own default).
        close()
        buttonEl.current?.focus()
        return
      case 'Enter':
      case ' ':
      case 'Spacebar': {
        // A bound row (an option, Exit Zen Mode) is K2's: the registry ran
        // its action and took the key. Any other row: one click.
        if (e.defaultPrevented) return
        const target = e.target as HTMLElement
        if (!target.matches(ITEM_SELECTOR)) return
        e.preventDefault()
        target.click()
        return
      }
    }
  }

  const gardens = bridge.gardens.list()
  const current = bridge.gardens.current()

  const section = (entry: ZenChromeItem): React.ReactNode => {
    switch (entry.kind) {
      case 'garden-switcher':
        return (
          <div role="group" aria-label="Gardens" data-zen-menu-section="garden-switcher" className="flex flex-col" style={{ gap: 2 }}>
            <SectionTitle>Gardens</SectionTitle>
            {gardens.map((g) => (
              <GardenChoice key={g.id} bridge={bridge} garden={g} selected={g.id === current?.id} onPicked={close} />
            ))}
            {bridge.caps.has('gardens:manage') && <ZenNewGarden bridge={bridge} onDone={close} />}
          </div>
        )
      case 'zen-toggle':
        return (
          <div data-zen-menu-section="zen-toggle">
            <ExitRow bridge={bridge} />
          </div>
        )
      case 'theme-picker': {
        if (!k2.theme || k2.theme.themes.length === 0) return null
        const active = k2.theme.active
        const name = active ? (k2.theme.themes.find((t) => t.name === active)?.label ?? zenThemeLabel(active)) : 'Theme'
        return (
          <div data-zen-menu-section="theme-picker">
            <PanelRow panel="theme" onOpen={() => openPanel('theme')}>
              Theme: {name}
            </PanelRow>
          </div>
        )
      }
      case 'usage':
        if (!k2.usage) return null
        return (
          <div data-zen-menu-section="usage">
            <PanelRow panel="usage" onOpen={() => openPanel('usage')}>
              Usage
            </PanelRow>
          </div>
        )
      default:
        return null
    }
  }

  const topPanel = (): React.ReactNode => {
    const sections = menuItems.map((m) => [m.id, section(m)] as const).filter(([, node]) => node !== null)
    return sections.map(([id, node], i) => (
      <div key={id} className="flex flex-col" style={{ gap: 2 }}>
        {i > 0 && <MenuSeparator />}
        {node}
      </div>
    ))
  }

  const themePanel = (): React.ReactNode => {
    const theme = k2.theme
    if (!theme) return null
    return (
      <>
        <BackRow onBack={back} />
        <MenuSeparator />
        {theme.themes.map((t) => {
          const on = t.name === theme.active
          return (
            <button
              key={t.name}
              type="button"
              role="menuitemradio"
              aria-checked={on}
              data-zen-menu-theme={t.name}
              onClick={() => {
                theme.onPick(t.name)
                close()
              }}
              className="flex w-full items-center gap-3 text-left cursor-pointer"
              style={{ ...ROW_STYLE, background: on ? 'var(--zen-surface)' : 'transparent' }}
            >
              <span aria-hidden style={{ width: 12, color: 'var(--zen-accent)' }}>
                {on ? '✓' : ''}
              </span>
              <span className="min-w-0 flex-1 truncate">{t.label}</span>
              {t.builtin && <span style={{ fontSize: '0.8em', color: 'var(--zen-text-muted)' }}>built in</span>}
            </button>
          )
        })}
        {theme.error && (
          <div role="alert" style={{ padding: '6px 10px', color: 'var(--zen-danger)' }}>
            {theme.error}
          </div>
        )}
      </>
    )
  }

  const usagePanel = (): React.ReactNode => (
    <>
      <BackRow onBack={back} />
      <MenuSeparator />
      <div data-zen-menu-usage="" style={{ padding: '4px 6px' }}>
        <ZenUsageTool place={place} inline />
      </div>
    </>
  )

  return (
    <div ref={menu.anchorRef} className="relative" data-zen-menu={item.id} style={{ flexShrink: 0 }}>
      <button
        ref={buttonRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label={label}
        title={label}
        data-zen-menu-button={item.id}
        data-zen-soft-button=""
        {...ZEN_GLASS_PROPS}
        onClick={() => {
          if (open) close()
          else {
            setPanel('top')
            setOpen(true)
          }
        }}
        onKeyDown={(e) => {
          // Enter and Space click the button natively (never cancelled,
          // FC25); ↓ opens it too.
          if (e.key === 'ArrowDown' && !open) {
            e.preventDefault()
            setPanel('top')
            setOpen(true)
          }
        }}
        className="flex items-center justify-center gap-2 cursor-pointer"
        style={{
          height: 30,
          minWidth: 30,
          padding: shownLabel ? '0 12px 0 10px' : 0,
          borderRadius: 999,
          color: 'var(--zen-text)',
          fontWeight: 600,
        }}
      >
        <MenuGlyph icon={menuIcon(item)} />
        {shownLabel && <span className="max-w-[10rem] truncate">{shownLabel}</span>}
      </button>
      {open &&
        menu.portal(
          <div
            ref={listRef}
            role="menu"
            aria-label={label}
            data-zen-menu-list={item.id}
            data-zen-menu-panel={panel}
            data-zen-menu-placement={menu.placement}
            className="flex flex-col"
            onKeyDown={onListKey}
            style={{ ...menu.style, ...ZEN_DROPDOWN_STYLE, maxHeight: 'calc(100vh - 24px)', overflowY: 'auto' }}
          >
            {panel === 'theme' ? themePanel() : panel === 'usage' ? usagePanel() : topPanel()}
          </div>,
        )}
    </div>
  )
}
