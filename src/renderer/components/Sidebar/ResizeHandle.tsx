import { useCallback, useRef } from 'react'
import { useSidebarStore } from '@/stores/sidebar'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../../shared/constants'

export default function ResizeHandle(): React.JSX.Element {
  const setWidth = useSidebarStore((s) => s.setWidth)
  const isDragging = useRef(false)

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      isDragging.current = true

      const startX = e.clientX
      const startWidth = useSidebarStore.getState().width

      const handleMouseMove = (moveEvent: MouseEvent): void => {
        if (!isDragging.current) return
        const delta = moveEvent.clientX - startX
        const newWidth = Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, startWidth + delta))
        setWidth(newWidth)
      }

      const handleMouseUp = (): void => {
        isDragging.current = false
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    },
    [setWidth]
  )

  const handleDoubleClick = useCallback(() => {
    setWidth(SIDEBAR_DEFAULT_WIDTH)
  }, [setWidth])

  return (
    <div
      className="no-drag absolute top-0 right-0 bottom-0 w-1 cursor-col-resize hover:bg-[var(--color-accent)] transition-colors duration-150 z-10"
      onMouseDown={handleMouseDown}
      onDoubleClick={handleDoubleClick}
    />
  )
}
