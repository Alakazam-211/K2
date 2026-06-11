import { useEffect, useRef, useCallback } from 'react'
import { useContextMenuStore } from '../../stores/context-menu'

export default function ContextMenu(): React.JSX.Element | null {
  const isOpen = useContextMenuStore((s) => s.isOpen)
  const x = useContextMenuStore((s) => s.x)
  const y = useContextMenuStore((s) => s.y)
  const items = useContextMenuStore((s) => s.items)
  const focusedIndex = useContextMenuStore((s) => s.focusedIndex)
  const close = useContextMenuStore((s) => s.close)
  const selectItem = useContextMenuStore((s) => s.selectItem)
  const setFocusedIndex = useContextMenuStore((s) => s.setFocusedIndex)

  const menuRef = useRef<HTMLDivElement>(null)

  // Selectable (non-separator, enabled) item indices
  const selectableIndices = items
    .map((item, i) => (item.type !== 'separator' && item.enabled !== false ? i : -1))
    .filter((i) => i !== -1)

  // Position adjustment to keep menu within viewport
  const adjustedPosition = useCallback(() => {
    if (!menuRef.current) return { left: x, top: y }
    const rect = menuRef.current.getBoundingClientRect()
    const vw = window.innerWidth
    const vh = window.innerHeight

    let left = x
    let top = y

    if (left + rect.width > vw) {
      left = vw - rect.width - 4
    }
    if (top + rect.height > vh) {
      top = vh - rect.height - 4
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

      if (e.key === 'Enter') {
        e.preventDefault()
        e.stopPropagation()
        if (focusedIndex >= 0 && focusedIndex < items.length) {
          const item = items[focusedIndex]
          if (item.type !== 'separator' && item.enabled !== false) {
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
        className="no-drag"
        style={{
        position: 'fixed',
        left: x,
        top: y,
        zIndex: 99999,
        minWidth: 180,
        maxWidth: 320,
        background: 'var(--color-bg-surface)',
        border: '1px solid var(--color-border)',
        boxShadow: '0 4px 24px rgba(0, 0, 0, 0.5), 0 1px 4px rgba(0, 0, 0, 0.3)',
        padding: '4px 0',
        fontFamily:
          "'MesloLGM Nerd Font', Menlo, Monaco, 'Cascadia Code', 'Fira Code', 'SF Mono', Consolas, monospace",
        opacity: 1,
        animation: 'context-menu-fade-in 50ms ease-out'
      }}
    >
      {items.map((item, index) => {
        if (item.type === 'separator') {
          return (
            <div
              key={item.id || `sep-${index}`}
              style={{
                height: 1,
                margin: '4px 8px',
                background: 'var(--color-border)'
              }}
            />
          )
        }

        const isDisabled = item.enabled === false
        const isFocused = focusedIndex === index

        return (
          <button
            key={item.id}
            style={{
              display: 'block',
              width: '100%',
              padding: '5px 12px',
              border: 'none',
              background: isFocused ? 'rgba(255, 255, 255, 0.08)' : 'transparent',
              color: isDisabled
                ? 'var(--color-text-muted)'
                : 'var(--color-text-secondary)',
              fontSize: '12px',
              fontFamily: 'inherit',
              textAlign: 'left',
              cursor: isDisabled ? 'default' : 'pointer',
              opacity: isDisabled ? 0.5 : 1,
              lineHeight: '1.4',
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis'
            }}
            onMouseEnter={() => {
              if (!isDisabled) setFocusedIndex(index)
            }}
            onMouseLeave={() => {
              if (focusedIndex === index) setFocusedIndex(-1)
            }}
            onClick={(e) => {
              e.stopPropagation()
              if (!isDisabled) selectItem(item.id)
            }}
            disabled={isDisabled}
          >
            {item.label}
          </button>
        )
      })}
      </div>
    </>
  )
}
