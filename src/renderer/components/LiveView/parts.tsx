// Small shared pieces of the live workbook view: avatars and the few icons
// the K2 set doesn't have (drawn the K2 way: 24-unit box, currentColor).

import type { Actor } from '@/lib/live-view/wording'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'

export function Avatar({ actor, size = 18, ring = false, away = false }: { actor: Pick<Actor, 'name' | 'kind' | 'colour'>; size?: number; ring?: boolean; away?: boolean }): React.JSX.Element {
  return (
    <span
      className="lv-av"
      data-agent={actor.kind === 'agent' ? '' : undefined}
      data-ring={ring ? '' : undefined}
      data-away={away ? '' : undefined}
      style={{ ['--c' as string]: actor.colour, width: size, height: size, fontSize: Math.round(size * 0.5) }}
      title={`${actor.name} (${actor.kind === 'file' ? 'the file' : actor.kind})`}
      aria-hidden="true"
    >
      {actor.kind === 'file' ? <DocGlyph size={Math.round(size * 0.62)} /> : (actor.name[0] ?? '?').toUpperCase()}
    </span>
  )
}

function Svg({ size = 14, children, fill = false }: { size?: number; children: React.ReactNode; fill?: boolean }): React.JSX.Element {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill={fill ? 'currentColor' : 'none'} stroke={fill ? 'none' : 'currentColor'} strokeWidth={K2_ICON_STROKE} strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
      {children}
    </svg>
  )
}

export const DocGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <path d="M6 3.5h8l4 4v13H6z" />
    <path d="M14 3.5v4h4" />
  </Svg>
)
export const SheetGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <rect x="3.5" y="4" width="17" height="16" rx="1.5" />
    <path d="M3.5 9h17M3.5 14.5h17M9.5 4v16" />
  </Svg>
)
export const FollowGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <circle cx="12" cy="12" r="7.5" />
    <circle cx="12" cy="12" r="2.5" />
    <path d="M12 2v3M12 19v3M2 12h3M19 12h3" />
  </Svg>
)
export const EyeOffGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <path d="M3 3l18 18M10.6 6.1A9.8 9.8 0 0 1 12 6c5.5 0 9 6 9 6a15 15 0 0 1-2.6 3.3M6.6 6.6C4.3 8.1 3 12 3 12s3.5 6 9 6c1.5 0 2.8-.4 4-1" />
  </Svg>
)
export const PlayGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size} fill>
    <path d="M7 4.5v15l12.5-7.5z" />
  </Svg>
)
export const PauseGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size} fill>
    <rect x="6" y="4.5" width="4" height="15" rx="1" />
    <rect x="14" y="4.5" width="4" height="15" rx="1" />
  </Svg>
)
export const StepGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <path d="M6 5l9 7-9 7z" />
    <path d="M18 5v14" />
  </Svg>
)
export const RestartGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <path d="M4 12a8 8 0 1 0 2.3-5.7" />
    <path d="M4 4v4h4" />
  </Svg>
)
export const InfoGlyph = ({ size }: { size?: number }): React.JSX.Element => (
  <Svg size={size}>
    <circle cx="12" cy="12" r="9" />
    <path d="M12 11v5.5M12 7.6h.01" />
  </Svg>
)
export const ArrowGlyph = ({ dir, size }: { dir: 'up' | 'down' | 'left' | 'right'; size?: number }): React.JSX.Element => (
  <Svg size={size}>
    {dir === 'up' ? <path d="M12 19V5M6 11l6-6 6 6" /> : dir === 'down' ? <path d="M12 5v14M6 13l6 6 6-6" /> : dir === 'left' ? <path d="M19 12H5M11 6l-6 6 6 6" /> : <path d="M5 12h14M13 6l6 6-6 6" />}
  </Svg>
)

/** Seconds into the sample's timeline, for history times. */
export function secondsFrom(t0: number | null, at: number | null): string {
  if (t0 == null || at == null) return ''
  return `${((at - t0) / 1000).toFixed(1)} s`
}
