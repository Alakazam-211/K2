// The axis compass in the picture's corner: X red, Y green, Z blue, from the
// still's own camera, so it turns with each view and turntable frame. Far
// axes draw faded, under the near ones. One small disc in every pane.

import { compassAxes, type StillCamera } from '@/lib/live-view/scene/project'

const L = 15
const C = 22

export function Compass({ camera }: { camera: StillCamera }): React.JSX.Element {
  const ax = compassAxes(camera)
  return (
    <div className="lv-compass lv-plate" data-k2-chamfer="chip" title="Which way this picture faces: X red, Y green, Z blue (up)" data-testid="lv-scene-compass">
      <svg viewBox="0 0 44 44" aria-hidden="true">
        {ax.map((a) => {
          const x = C + a.dx * L
          const y = C + a.dy * L
          return (
            <g key={a.axis} data-axis={a.axis} className={a.away > 0.3 ? 'lv-far' : undefined}>
              <line x1={C} y1={C} x2={x} y2={y} />
              <circle className="lv-ax" cx={x} cy={y} r={4.6} />
              <text x={x} y={y + 2.5}>
                {a.axis.toUpperCase()}
              </text>
            </g>
          )
        })}
        <circle className="lv-o" cx={C} cy={C} r={1.6} />
      </svg>
    </div>
  )
}
