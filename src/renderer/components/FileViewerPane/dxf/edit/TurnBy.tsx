// The panel's exact turns, about the selection's centre: "Rotate by" a
// number of degrees (anticlockwise), and a quarter turn either way.

import { useState } from 'react'
import { Button, Input } from '@/components/ui'
import type { CadEdit } from './useCadEdit'

const LABEL = 'type-hint text-[var(--color-text-secondary)]'

/** A typed number, or null when it isn't one ("" is null). */
export const typedNumber = (v: string): number | null => {
  const t = v.trim().replace(/^\+/, '')
  if (!t || !/^-?(\d+\.?\d*|\.\d+)$/.test(t)) return null
  return Number(t)
}

export function RotateBy({ edit }: { edit: CadEdit }): React.JSX.Element {
  const [v, setV] = useState('')
  const deg = typedNumber(v)
  const ok = deg !== null && deg % 360 !== 0
  const off = !edit.editable || edit.busy
  const go = (d: number, clear = false) => {
    if (off) return
    void edit.rotate(d).then((landed) => {
      if (landed && clear) setV('')
    })
  }
  return (
    <form
      onSubmit={(e) => {
        e.preventDefault()
        if (ok) go(deg, true)
      }}
      className="pt-2"
      data-testid="dxf-rotate-by"
    >
      <p className={`${LABEL} m-0 pb-1`}>Rotate by, in degrees anticlockwise</p>
      <div className="flex items-center gap-2">
        <Input inputMode="decimal" value={v} placeholder="0" onChange={(e) => setV(e.target.value)} className="w-full min-w-0 flex-1" aria-label="Rotate by, in degrees anticlockwise" data-testid="dxf-rotate-deg" />
        <Button type="submit" size="sm" variant="default" disabled={!ok || off} data-testid="dxf-rotate-go">
          Rotate
        </Button>
      </div>
      <div className="flex items-center gap-2 pt-1.5">
        <Button type="button" size="sm" variant="quiet" disabled={off} onClick={() => go(90)} aria-label="Rotate 90° left (anticlockwise)" data-testid="dxf-rotate-left">
          90° left
        </Button>
        <Button type="button" size="sm" variant="quiet" disabled={off} onClick={() => go(-90)} aria-label="Rotate 90° right (clockwise)" data-testid="dxf-rotate-right">
          90° right
        </Button>
      </div>
      <p className={`${LABEL} m-0 pt-1`}>About the selection’s centre.</p>
    </form>
  )
}
