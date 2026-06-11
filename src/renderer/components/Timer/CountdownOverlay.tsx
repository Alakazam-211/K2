import { useState, useEffect, useRef, useCallback } from 'react'
import { useTimerStore, type CountdownThemeConfig } from '@/stores/timer'

const COUNTDOWN_FROM = 3

export default function CountdownOverlay(): React.JSX.Element | null {
  const showCountdown = useTimerStore((s) => s.showCountdown)
  const countdownTheme = useTimerStore((s) => s.countdownTheme)
  const customThemes = useTimerStore((s) => s.customThemes)
  const startTimer = useTimerStore((s) => s.startTimer)
  const cancelCountdown = useTimerStore((s) => s.cancelCountdown)

  const [count, setCount] = useState(COUNTDOWN_FROM)
  const [phase, setPhase] = useState<'counting' | 'final'>('counting')

  // Reset on show
  useEffect(() => {
    if (showCountdown) {
      setCount(COUNTDOWN_FROM)
      setPhase('counting')
    }
  }, [showCountdown])

  // Countdown logic
  useEffect(() => {
    if (!showCountdown || phase !== 'counting') return
    if (count <= 0) {
      setPhase('final')
      return
    }
    const timer = setTimeout(() => setCount((c) => c - 1), 1000)
    return () => clearTimeout(timer)
  }, [showCountdown, count, phase])

  // Final text display, then start
  useEffect(() => {
    if (!showCountdown || phase !== 'final') return
    const timer = setTimeout(() => {
      startTimer()
    }, 800)
    return () => clearTimeout(timer)
  }, [showCountdown, phase, startTimer])

  // Escape to cancel
  useEffect(() => {
    if (!showCountdown) return
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') cancelCountdown()
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [showCountdown, cancelCountdown])

  if (!showCountdown) return null

  // Find custom theme or use built-in
  const custom = customThemes.find((t) => t.name === countdownTheme) as CountdownThemeConfig | undefined

  if (custom) {
    return <CustomThemeRenderer config={custom} count={count} phase={phase} />
  }

  switch (countdownTheme) {
    case 'matrix':
      return <MatrixTheme count={count} phase={phase} />
    case 'retro':
      return <RetroTheme count={count} phase={phase} />
    case 'rocket':
    default:
      return <RocketTheme count={count} phase={phase} />
  }
}

// ── Rocket Launch Theme ────────────────────────────────────────────────────

function RocketTheme({ count, phase }: { count: number; phase: 'counting' | 'final' }): React.JSX.Element {
  return (
    <div
      className="fixed inset-0 z-50 flex flex-col items-center justify-center"
      style={{
        background: 'radial-gradient(ellipse at 50% 100%, #0a0a2e 0%, #050510 70%, #000 100%)',
      }}
    >
      {/* Stars */}
      <div className="absolute inset-0 overflow-hidden pointer-events-none">
        {Array.from({ length: 60 }).map((_, i) => (
          <div
            key={i}
            className="absolute rounded-full bg-white"
            style={{
              width: Math.random() * 2 + 1,
              height: Math.random() * 2 + 1,
              left: `${Math.random() * 100}%`,
              top: `${Math.random() * 100}%`,
              opacity: Math.random() * 0.7 + 0.3,
              animation: `twinkle ${2 + Math.random() * 3}s ease-in-out infinite`,
              animationDelay: `${Math.random() * 2}s`,
            }}
          />
        ))}
      </div>

      {phase === 'counting' ? (
        <>
          <div className="text-sm uppercase tracking-[0.4em] text-blue-300/60 mb-4 font-mono">
            T-minus
          </div>
          <div
            className="text-[120px] font-black text-white leading-none"
            style={{
              textShadow: '0 0 40px rgba(100, 150, 255, 0.5), 0 0 80px rgba(100, 150, 255, 0.2)',
              animation: 'countdown-pop 1s ease-out',
            }}
            key={count}
          >
            {count}
          </div>
        </>
      ) : (
        <div
          className="flex flex-col items-center"
          style={{ animation: 'liftoff-shake 0.6s ease-out' }}
        >
          <div
            className="text-5xl font-black tracking-wider text-orange-400"
            style={{
              textShadow: '0 0 30px rgba(255, 150, 50, 0.6), 0 0 60px rgba(255, 100, 0, 0.3)',
            }}
          >
            LIFTOFF!
          </div>
          <div className="text-4xl mt-4" style={{ animation: 'rocket-fly 0.8s ease-in forwards' }}>
            🚀
          </div>
        </div>
      )}

      <style>{`
        @keyframes twinkle {
          0%, 100% { opacity: 0.3; }
          50% { opacity: 1; }
        }
        @keyframes countdown-pop {
          0% { transform: scale(1.6); opacity: 0; }
          40% { transform: scale(0.95); opacity: 1; }
          100% { transform: scale(1); }
        }
        @keyframes liftoff-shake {
          0%, 100% { transform: translateX(0); }
          10% { transform: translateX(-4px) translateY(2px); }
          20% { transform: translateX(4px) translateY(-2px); }
          30% { transform: translateX(-3px) translateY(1px); }
          40% { transform: translateX(3px) translateY(-1px); }
          50% { transform: translateX(-2px); }
          60% { transform: translateX(2px); }
        }
        @keyframes rocket-fly {
          0% { transform: translateY(0); opacity: 1; }
          100% { transform: translateY(-200px); opacity: 0; }
        }
      `}</style>
    </div>
  )
}

// ── Matrix Rain Theme ──────────────────────────────────────────────────────

function MatrixTheme({ count, phase }: { count: number; phase: 'counting' | 'final' }): React.JSX.Element {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const ctx = canvas.getContext('2d')
    if (!ctx) return

    canvas.width = window.innerWidth
    canvas.height = window.innerHeight

    const columns = Math.floor(canvas.width / 20)
    const drops: number[] = Array(columns).fill(1)
    const chars = 'アイウエオカキクケコサシスセソタチツテトナニヌネノハヒフヘホマミムメモヤユヨラリルレロワヲン0123456789'

    let animId: number
    const draw = () => {
      ctx.fillStyle = 'rgba(0, 0, 0, 0.05)'
      ctx.fillRect(0, 0, canvas.width, canvas.height)
      ctx.fillStyle = '#0f0'
      ctx.font = '14px monospace'

      for (let i = 0; i < drops.length; i++) {
        const char = chars[Math.floor(Math.random() * chars.length)]
        ctx.fillText(char, i * 20, drops[i] * 20)
        if (drops[i] * 20 > canvas.height && Math.random() > 0.975) {
          drops[i] = 0
        }
        drops[i]++
      }
      animId = requestAnimationFrame(draw)
    }
    draw()
    return () => cancelAnimationFrame(animId)
  }, [])

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black">
      <canvas ref={canvasRef} className="absolute inset-0" />
      <div className="relative z-10 text-center">
        {phase === 'counting' ? (
          count === COUNTDOWN_FROM ? (
            <div
              className="text-lg font-mono text-green-400/70 tracking-widest"
              style={{ animation: 'matrix-fade-in 0.8s ease-out' }}
            >
              Wake up...
            </div>
          ) : (
            <div
              className="text-[120px] font-mono font-black text-green-400 leading-none"
              style={{
                textShadow: '0 0 30px rgba(0, 255, 0, 0.5), 0 0 60px rgba(0, 255, 0, 0.2)',
                animation: 'countdown-pop 1s ease-out',
              }}
              key={count}
            >
              {count}
            </div>
          )
        ) : (
          <div
            className="text-4xl font-mono font-black tracking-[0.2em] text-green-400"
            style={{
              textShadow: '0 0 20px rgba(0, 255, 0, 0.6)',
              animation: 'matrix-fade-in 0.5s ease-out',
            }}
          >
            ENTER THE FLOW
          </div>
        )}
      </div>

      <style>{`
        @keyframes matrix-fade-in {
          0% { opacity: 0; transform: translateY(10px); }
          100% { opacity: 1; transform: translateY(0); }
        }
        @keyframes countdown-pop {
          0% { transform: scale(1.6); opacity: 0; }
          40% { transform: scale(0.95); opacity: 1; }
          100% { transform: scale(1); }
        }
      `}</style>
    </div>
  )
}

// ── Retro Arcade Theme ─────────────────────────────────────────────────────

function RetroTheme({ count, phase }: { count: number; phase: 'counting' | 'final' }): React.JSX.Element {
  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{
        background: '#0a0a0a',
      }}
    >
      {/* Scanlines */}
      <div
        className="absolute inset-0 pointer-events-none opacity-20"
        style={{
          background: 'repeating-linear-gradient(0deg, transparent, transparent 2px, rgba(0,0,0,0.3) 2px, rgba(0,0,0,0.3) 4px)',
        }}
      />

      {/* CRT vignette */}
      <div
        className="absolute inset-0 pointer-events-none"
        style={{
          background: 'radial-gradient(ellipse at center, transparent 60%, rgba(0,0,0,0.7) 100%)',
        }}
      />

      <div className="relative z-10 text-center">
        {phase === 'counting' ? (
          count === COUNTDOWN_FROM ? (
            <div
              className="text-4xl font-mono font-black tracking-[0.3em]"
              style={{
                color: '#39ff14',
                textShadow: '0 0 10px #39ff14, 0 0 20px #39ff14, 0 0 40px rgba(57,255,20,0.3)',
                animation: 'retro-blink 0.6s ease-in-out',
              }}
            >
              READY?
            </div>
          ) : (
            <div
              className="text-[140px] font-mono font-black leading-none"
              style={{
                color: '#ff2d95',
                textShadow: '0 0 15px #ff2d95, 0 0 30px #ff2d95, 4px 4px 0 #39ff14',
                animation: 'retro-pop 1s ease-out',
              }}
              key={count}
            >
              {count}
            </div>
          )
        ) : (
          <div
            className="text-6xl font-mono font-black tracking-[0.3em]"
            style={{
              color: '#39ff14',
              textShadow: '0 0 15px #39ff14, 0 0 30px #39ff14, 0 0 60px rgba(57,255,20,0.4)',
              animation: 'retro-flash 0.5s ease-out',
            }}
          >
            GO!
          </div>
        )}
      </div>

      <style>{`
        @keyframes retro-blink {
          0%, 40% { opacity: 0; }
          50%, 100% { opacity: 1; }
        }
        @keyframes retro-pop {
          0% { transform: scale(2); opacity: 0; }
          30% { transform: scale(0.9); opacity: 1; }
          100% { transform: scale(1); }
        }
        @keyframes retro-flash {
          0% { transform: scale(0.5); opacity: 0; }
          50% { transform: scale(1.2); opacity: 1; }
          100% { transform: scale(1); }
        }
      `}</style>
    </div>
  )
}

// ── Custom Theme Renderer ──────────────────────────────────────────────────

function CustomThemeRenderer({
  config,
  count,
  phase,
}: {
  config: CountdownThemeConfig
  count: number
  phase: 'counting' | 'final'
}): React.JSX.Element {
  const animClass = {
    fade: 'custom-fade',
    zoom: 'custom-zoom',
    slide: 'custom-slide',
  }[config.animationPreset] ?? 'custom-fade'

  const text = phase === 'final'
    ? config.finalText
    : config.countdownTexts?.[COUNTDOWN_FROM - count] ?? String(count)

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={{
        backgroundColor: config.backgroundColor || '#0a0a0a',
        fontFamily: config.fontFamily || 'monospace',
      }}
    >
      <div
        className={`text-[100px] font-black leading-none ${animClass}`}
        style={{ color: config.textColor || '#ffffff' }}
        key={`${phase}-${count}`}
      >
        {text}
      </div>

      <style>{`
        .custom-fade {
          animation: custom-fade-in 0.8s ease-out;
        }
        .custom-zoom {
          animation: custom-zoom-in 0.8s ease-out;
        }
        .custom-slide {
          animation: custom-slide-in 0.8s ease-out;
        }
        @keyframes custom-fade-in {
          0% { opacity: 0; }
          100% { opacity: 1; }
        }
        @keyframes custom-zoom-in {
          0% { transform: scale(2); opacity: 0; }
          100% { transform: scale(1); opacity: 1; }
        }
        @keyframes custom-slide-in {
          0% { transform: translateY(40px); opacity: 0; }
          100% { transform: translateY(0); opacity: 1; }
        }
      `}</style>
    </div>
  )
}
