import { useEffect, useRef, useCallback, useLayoutEffect, useState } from 'react'
import { KeyCombo } from '@/components/KeySymbol'
import {
  clientToCssPx,
  isSelectableItem,
  useContextMenuStore,
  type ContextMenuItemDef,
} from '../../stores/context-menu'

const MENU_FONT =
  "'MesloLGM Nerd Font', Menlo, Monaco, 'Cascadia Code', 'Fira Code', 'SF Mono', Consolas, monospace"

const PANEL_STYLE: React.CSSProperties = {
  minWidth: 180,
  maxWidth: 320,
  background: 'var(--color-bg)',
  border: '1px solid var(--color-border)',
  boxShadow: '0 4px 24px rgba(0, 0, 0, 0.5), 0 1px 4px rgba(0, 0, 0, 0.3)',
  padding: '4px 0',
  fontFamily: MENU_FONT,
  opacity: 1,
}

/** Keyboard can land on any pickable row and on a submenu parent. */
function isFocusable(item: ContextMenuItemDef): boolean {
  return isSelectableItem(item) || (Boolean(item.submenu) && item.enabled !== false)
}

/** A separator, heading or note: not a button. */
function StaticRow({ item, index }: { item: ContextMenuItemDef; index: number }): React.JSX.Element | null {
  if (item.type === 'separator') {
    return (
      <div
        key={item.id || `sep-${index}`}
        style={{ height: 1, margin: '4px 8px', background: 'var(--color-border)' }}
      />
    )
  }
  if (item.type === 'heading') {
    return (
      <div
        role="presentation"
        data-context-menu-heading=""
        style={{
          padding: '6px 12px 2px',
          fontSize: '9px',
          letterSpacing: '0.06em',
          textTransform: 'uppercase',
          color: 'var(--color-text-muted)',
        }}
      >
        {item.label}
      </div>
    )
  }
  if (item.type === 'note') {
    return (
      <div
        role="note"
        style={{
          padding: '4px 12px',
          fontSize: '10px',
          lineHeight: 1.4,
          color: 'var(--color-text-muted)',
          whiteSpace: 'normal',
          maxWidth: 280,
        }}
      >
        {item.label}
      </div>
    )
  }
  return null
}

function RowButton({
  item,
  focused,
  open,
  onEnter,
  onLeave,
  onClick,
}: {
  item: ContextMenuItemDef
  focused: boolean
  open?: boolean
  onEnter: () => void
  onLeave: () => void
  onClick: () => void
}): React.JSX.Element {
  const isDisabled = item.enabled === false
  const hasSub = Boolean(item.submenu)
  return (
    <button
      type="button"
      aria-haspopup={hasSub ? 'menu' : undefined}
      aria-expanded={hasSub ? Boolean(open) : undefined}
      aria-pressed={item.checked !== undefined ? item.checked : undefined}
      data-context-menu-item={item.id}
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 8,
        width: '100%',
        padding: '5px 12px',
        border: 'none',
        background: focused || open ? 'var(--color-overlay-soft-border)' : 'transparent',
        color: isDisabled ? 'var(--color-text-muted)' : 'var(--color-text-secondary)',
        fontSize: '12px',
        fontFamily: 'inherit',
        textAlign: 'left',
        cursor: isDisabled ? 'default' : 'pointer',
        opacity: isDisabled ? 0.5 : 1,
        lineHeight: '1.4',
        whiteSpace: 'nowrap',
        overflow: 'hidden',
      }}
      onMouseEnter={() => {
        if (!isDisabled) onEnter()
      }}
      onMouseLeave={onLeave}
      onClick={(e) => {
        e.stopPropagation()
        if (!isDisabled) onClick()
      }}
      disabled={isDisabled}
    >
      {item.checked !== undefined && (
        <span
          aria-hidden
          style={{
            width: 12,
            height: 12,
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
            border: '1px solid var(--color-border)',
            background: item.checked ? 'var(--color-accent)' : 'transparent',
            color: item.checked ? 'var(--color-on-accent)' : 'transparent',
            fontSize: 9,
            lineHeight: 1,
          }}
        >
          {item.checked ? '✓' : ''}
        </span>
      )}
      {item.icon ? (
        <span
          aria-hidden
          style={{
            width: 14,
            height: 14,
            flexShrink: 0,
            display: 'inline-flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          {item.icon}
        </span>
      ) : null}
      <span style={{ flex: 1, minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis' }}>{item.label}</span>
      {item.hint ? (
        <span style={{ flexShrink: 0, marginLeft: 12, color: 'var(--color-text-muted)', fontSize: '10px' }}>
          {item.hint}
        </span>
      ) : item.shortcut ? (
        <span style={{ flexShrink: 0, marginLeft: 12, color: 'var(--color-text-muted)', fontSize: '10px' }}>
          <KeyCombo combo={item.shortcut} />
        </span>
      ) : null}
      {item.badge ? (
        <span
          style={{
            flexShrink: 0,
            fontSize: '9px',
            fontWeight: 600,
            letterSpacing: '0.04em',
            textTransform: 'uppercase',
            lineHeight: 1.2,
            padding: '2px 5px',
            borderRadius: 3,
            color: 'var(--color-on-accent, #ffffff)',
            background: 'var(--color-accent, #3b82f6)',
            border: '1px solid var(--color-accent, #3b82f6)',
          }}
        >
          {item.badge}
        </span>
      ) : null}
      {hasSub ? (
        <span aria-hidden style={{ flexShrink: 0, marginLeft: 8, color: 'var(--color-text-muted)', fontSize: '10px' }}>
          ▸
        </span>
      ) : null}
    </button>
  )
}

/** The rows beside a "▸" parent. Opens to the right, or to the left when
 *  the right edge would cut it off. */
function Submenu({
  items,
  onPick,
}: {
  items: ContextMenuItemDef[]
  onPick: (id: string) => void
}): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  const [focused, setFocused] = useState(-1)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    if (clientToCssPx(rect.right) > window.innerWidth) {
      el.style.left = 'auto'
      el.style.right = '100%'
    }
    if (clientToCssPx(rect.bottom) > window.innerHeight) {
      el.style.top = 'auto'
      el.style.bottom = '-4px'
    }
  }, [items])
  return (
    <div
      ref={ref}
      role="menu"
      data-context-submenu=""
      style={{ ...PANEL_STYLE, position: 'absolute', left: '100%', top: -4, zIndex: 1 }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      {items.map((item, i) =>
        item.type === 'separator' || item.type === 'heading' || item.type === 'note' ? (
          <StaticRow key={item.id || `sub-${i}`} item={item} index={i} />
        ) : (
          <RowButton
            key={item.id}
            item={item}
            focused={focused === i}
            onEnter={() => setFocused(i)}
            onLeave={() => setFocused((f) => (f === i ? -1 : f))}
            onClick={() => {
              if (isSelectableItem(item)) onPick(item.id)
            }}
          />
        ),
      )}
    </div>
  )
}

export default function ContextMenu(): React.JSX.Element | null {
  const isOpen = useContextMenuStore((s) => s.isOpen)
  const x = useContextMenuStore((s) => s.x)
  const y = useContextMenuStore((s) => s.y)
  const items = useContextMenuStore((s) => s.items)
  const focusedIndex = useContextMenuStore((s) => s.focusedIndex)
  const close = useContextMenuStore((s) => s.close)
  const selectItem = useContextMenuStore((s) => s.selectItem)
  const setFocusedIndex = useContextMenuStore((s) => s.setFocusedIndex)
  const [openSub, setOpenSub] = useState<number | null>(null)

  const menuRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    setOpenSub(null)
  }, [isOpen, items])

  // Focusable (pickable or submenu parent) item indices
  const selectableIndices = items
    .map((item, i) => (isFocusable(item) ? i : -1))
    .filter((i) => i !== -1)

  // Position adjustment to keep menu within viewport.
  // Stored x/y and innerWidth are CSS px; gBCR is post-zoom.
  const adjustedPosition = useCallback(() => {
    if (!menuRef.current) return { left: x, top: y }
    const rect = menuRef.current.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight
    const width = clientToCssPx(rect.width)
    const height = clientToCssPx(rect.height)

    let left = x
    let top = y

    if (left + width > vw) {
      left = vw - width - 4
    }
    if (top + height > vh) {
      top = vh - height - 4
    }
    if (left < 0) left = 4
    if (top < 0) top = 4

    return { left, top }
  }, [x, y])

  // Keyboard navigation
  useEffect(() => {
    if (!isOpen) return

    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        close()
        return
      }

      if (e.key === 'ArrowDown') {
        e.preventDefault()
        e.stopPropagation()
        const currentPos = selectableIndices.indexOf(focusedIndex)
        const nextPos =
          currentPos < 0 || currentPos >= selectableIndices.length - 1
            ? 0
            : currentPos + 1
        setFocusedIndex(selectableIndices[nextPos])
        return
      }

      if (e.key === 'ArrowUp') {
        e.preventDefault()
        e.stopPropagation()
        const currentPos = selectableIndices.indexOf(focusedIndex)
        const nextPos =
          currentPos <= 0
            ? selectableIndices.length - 1
            : currentPos - 1
        setFocusedIndex(selectableIndices[nextPos])
        return
      }

      if (e.key === 'Enter' || e.key === 'ArrowRight') {
        if (focusedIndex >= 0 && focusedIndex < items.length) {
          const item = items[focusedIndex]
          if (item.submenu && item.enabled !== false) {
            e.preventDefault()
            e.stopPropagation()
            setOpenSub(focusedIndex)
            return
          }
          if (e.key === 'Enter' && isSelectableItem(item)) {
            e.preventDefault()
            e.stopPropagation()
            selectItem(item.id)
          }
        }
        return
      }
    }

    window.addEventListener('keydown', handleKeyDown, true)
    return () => window.removeEventListener('keydown', handleKeyDown, true)
  }, [isOpen, focusedIndex, items, selectableIndices, close, selectItem, setFocusedIndex])

  // Backup: window-level mousedown to catch clicks in drag regions
  // that the backdrop div might not receive
  useEffect(() => {
    if (!isOpen) return

    const handler = (e: MouseEvent): void => {
      if (menuRef.current && !menuRef.current.contains(e.target as Node)) {
        close()
      }
    }

    // Use a small delay so the opening right-click doesn't immediately close it
    const timer = setTimeout(() => {
      window.addEventListener('mousedown', handler, true)
    }, 50)

    return () => {
      clearTimeout(timer)
      window.removeEventListener('mousedown', handler, true)
    }
  }, [isOpen, close])

  // Reposition when menu mounts or items change
  useEffect(() => {
    if (!isOpen || !menuRef.current) return
    const { left, top } = adjustedPosition()
    menuRef.current.style.left = `${left}px`
    menuRef.current.style.top = `${top}px`
  }, [isOpen, items, adjustedPosition])

  if (!isOpen) return null

  return (
    <>
      {/* Invisible backdrop — click anywhere to dismiss */}
      <div
        data-context-backdrop=""
        style={{
          position: 'fixed',
          inset: 0,
          zIndex: 99998
        }}
        onMouseDown={(e) => {
          e.stopPropagation()
          close()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
          close()
        }}
      />
      <div
        ref={menuRef}
        data-context-menu=""
        className="no-drag"
        style={{
          ...PANEL_STYLE,
          position: 'fixed',
          left: x,
          top: y,
          zIndex: 99999,
          animation: 'context-menu-fade-in 50ms ease-out',
        }}
      >
        {items.map((item, index) => {
          if (item.type === 'separator' || item.type === 'heading' || item.type === 'note') {
            return <StaticRow key={item.id || `sep-${index}`} item={item} index={index} />
          }
          const row = (
            <RowButton
              key={item.submenu ? undefined : item.id}
              item={item}
              focused={focusedIndex === index}
              open={openSub === index}
              onEnter={() => {
                setFocusedIndex(index)
                setOpenSub(item.submenu ? index : null)
              }}
              onLeave={() => {
                if (focusedIndex === index) setFocusedIndex(-1)
              }}
              onClick={() => {
                if (item.submenu) setOpenSub((cur) => (cur === index ? null : index))
                else selectItem(item.id)
              }}
            />
          )
          if (!item.submenu) return row
          return (
            <div key={item.id} style={{ position: 'relative' }}>
              {row}
              {openSub === index && <Submenu items={item.submenu} onPick={selectItem} />}
            </div>
          )
        })}
      </div>
    </>
  )
}
