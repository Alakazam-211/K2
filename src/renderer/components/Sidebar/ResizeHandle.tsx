import { useCallback, useRef } from 'react'
import { displayedSidebarWidth, useSidebarStore } from '@/stores/sidebar'
import { isSetupChecklistShowing } from '@/stores/first-run'
import { SIDEBAR_DEFAULT_WIDTH } from '../../../shared/constants'

export default function ResizeHandle(): React.JSX.Element {
  const setWidth = useSidebarStore((s) => s.setWidth)
  const isDragging = useRef(false)

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      isDragging.current = true

      const startX = e.clientX
      // From the width on screen (the checklist's floor may be drawn over a
      // narrower saved width); the store clamps and keeps the saved width.
      const startWidth = displayedSidebarWidth(useSidebarStore.getState().width, isSetupChecklistShowing())

      const handleMouseMove = (moveEvent: MouseEvent): void => {
        if (!isDragging.current) return
        const delta = moveEvent.clientX - startX
        setWidth(startWidth + delta)
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
      data-resize-edge=""
      className="no-drag absolute top-0 right-0 bottom-0 w-1 cursor-col-resize hover:bg-[var(--color-accent)] transition-colors duration-150 z-10"
      onMouseDown={handleMouseDown}
      onDoubleClick={handleDoubleClick}
    />
  )
}
