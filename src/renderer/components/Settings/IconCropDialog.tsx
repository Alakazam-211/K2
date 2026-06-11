import { useState, useRef, useCallback, useEffect } from 'react'

interface IconCropDialogProps {
  imageDataUrl: string
  onConfirm: (croppedDataUrl: string) => void
  onCancel: () => void
}

const CROP_SIZE = 200 // preview square size in px
const OUTPUT_SIZE = 128 // final cropped image size in px

export default function IconCropDialog({
  imageDataUrl,
  onConfirm,
  onCancel
}: IconCropDialogProps): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const imgRef = useRef<HTMLImageElement | null>(null)

  // State: offset (pan) and scale (zoom)
  const [scale, setScale] = useState(1)
  const [offset, setOffset] = useState({ x: 0, y: 0 })
  const [dragging, setDragging] = useState(false)
  const dragStart = useRef({ x: 0, y: 0, ox: 0, oy: 0 })
  const [imgLoaded, setImgLoaded] = useState(false)

  // Load the image
  useEffect(() => {
    const img = new Image()
    img.onload = () => {
      imgRef.current = img
      // Initial scale: fit the smaller dimension to the crop square
      const s = CROP_SIZE / Math.min(img.width, img.height)
      setScale(s)
      // Center the image
      setOffset({
        x: (CROP_SIZE - img.width * s) / 2,
        y: (CROP_SIZE - img.height * s) / 2
      })
      setImgLoaded(true)
    }
    img.src = imageDataUrl
  }, [imageDataUrl])

  // Draw the preview
  useEffect(() => {
    const canvas = canvasRef.current
    const img = imgRef.current
    if (!canvas || !img || !imgLoaded) return

    const ctx = canvas.getContext('2d')
    if (!ctx) return

    canvas.width = CROP_SIZE
    canvas.height = CROP_SIZE

    ctx.clearRect(0, 0, CROP_SIZE, CROP_SIZE)
    ctx.fillStyle = '#0a0a0a'
    ctx.fillRect(0, 0, CROP_SIZE, CROP_SIZE)
    ctx.drawImage(img, offset.x, offset.y, img.width * scale, img.height * scale)
  }, [scale, offset, imgLoaded])

  // Mouse drag to pan
  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    setDragging(true)
    dragStart.current = { x: e.clientX, y: e.clientY, ox: offset.x, oy: offset.y }
  }, [offset])

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    if (!dragging) return
    setOffset({
      x: dragStart.current.ox + (e.clientX - dragStart.current.x),
      y: dragStart.current.oy + (e.clientY - dragStart.current.y)
    })
  }, [dragging])

  const handleMouseUp = useCallback(() => {
    setDragging(false)
  }, [])

  // Scroll to zoom
  const handleWheel = useCallback((e: React.WheelEvent) => {
    e.preventDefault()
    const img = imgRef.current
    if (!img) return

    const minScale = CROP_SIZE / Math.max(img.width, img.height) * 0.5
    const maxScale = 5
    const delta = e.deltaY > 0 ? 0.95 : 1.05
    const newScale = Math.max(minScale, Math.min(maxScale, scale * delta))

    // Zoom toward center of crop area
    const cx = CROP_SIZE / 2
    const cy = CROP_SIZE / 2
    const newX = cx - (cx - offset.x) * (newScale / scale)
    const newY = cy - (cy - offset.y) * (newScale / scale)

    setScale(newScale)
    setOffset({ x: newX, y: newY })
  }, [scale, offset])

  // Confirm: render the crop at output resolution
  const handleConfirm = useCallback(() => {
    const img = imgRef.current
    if (!img) return

    const outCanvas = document.createElement('canvas')
    outCanvas.width = OUTPUT_SIZE
    outCanvas.height = OUTPUT_SIZE
    const ctx = outCanvas.getContext('2d')
    if (!ctx) return

    // Map from preview coords to output coords
    const ratio = OUTPUT_SIZE / CROP_SIZE
    ctx.drawImage(
      img,
      offset.x * ratio,
      offset.y * ratio,
      img.width * scale * ratio,
      img.height * scale * ratio
    )

    onConfirm(outCanvas.toDataURL('image/png'))
  }, [scale, offset, onConfirm])

  // Escape to cancel
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onCancel()
      if (e.key === 'Enter') handleConfirm()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [onCancel, handleConfirm])

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.7)' }}
      onClick={onCancel}
    >
      <div
        className="flex flex-col items-center gap-4 p-6"
        style={{
          backgroundColor: '#111',
          border: '1px solid var(--color-border)',
        }}
        onClick={(e) => e.stopPropagation()}
      >
        <span className="text-xs text-[var(--color-text-muted)] font-mono tracking-wide uppercase">
          Drag to position, scroll to zoom
        </span>

        {/* Crop preview */}
        <canvas
          ref={canvasRef}
          width={CROP_SIZE}
          height={CROP_SIZE}
          style={{
            width: CROP_SIZE,
            height: CROP_SIZE,
            cursor: dragging ? 'grabbing' : 'grab',
            border: '1px solid var(--color-border)',
          }}
          onMouseDown={handleMouseDown}
          onMouseMove={handleMouseMove}
          onMouseUp={handleMouseUp}
          onMouseLeave={handleMouseUp}
          onWheel={handleWheel}
        />

        {/* Actions */}
        <div className="flex items-center gap-3">
          <button
            onClick={onCancel}
            className="px-4 py-1.5 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer transition-colors font-mono"
          >
            Cancel
          </button>
          <button
            onClick={handleConfirm}
            className="px-4 py-1.5 text-xs text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer transition-colors font-mono"
          >
            Apply
          </button>
        </div>
      </div>
    </div>
  )
}
