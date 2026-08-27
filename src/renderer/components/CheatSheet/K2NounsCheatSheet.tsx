import { useCallback, useEffect, useState } from 'react'
import { DialogScrim, Surface } from '@/components/ui'
import { K2_CHEAT_SHEET_INTRO, K2_CHEAT_SHEET_NOTES, K2_NOUN_GROUPS } from './k2Nouns'

const noDrag = {
  WebkitAppRegion: 'no-drag',
} as React.CSSProperties

function PaperNoteIcon(): React.JSX.Element {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 14 14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3.5 1.75h5.2L11.5 4.55V12.25H3.5z" />
      <path d="M8.7 1.75V4.55H11.5" />
      <path d="M5.2 7.1h4.2M5.2 9.35h3.2" />
    </svg>
  )
}

export default function K2NounsCheatSheet(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const close = useCallback(() => setOpen(false), [])

  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      close()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [open, close])

  return (
    <>
      <button
        type="button"
        data-testid="k2-nouns-cheat-sheet"
        onClick={() => setOpen(true)}
        className="flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag"
        style={noDrag}
        title="K2 cheat sheet"
        aria-label="K2 cheat sheet"
      >
        <PaperNoteIcon />
      </button>

      {open && (
        <>
          <DialogScrim
            onMouseDown={(e) => {
              e.stopPropagation()
              close()
            }}
          />
          <Surface
            role2="surface"
            elevation={5}
            className="no-drag"
            role="dialog"
            aria-modal="true"
            aria-labelledby="k2-cheat-sheet-title"
            style={{
              position: 'fixed',
              top: '50%',
              left: '50%',
              transform: 'translate(-50%, -50%)',
              zIndex: 99999,
              width: 'min(600px, 90vw)',
              maxHeight: '78vh',
              display: 'flex',
              flexDirection: 'column',
              overflow: 'hidden',
              fontFamily:
                "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'Inter', system-ui, sans-serif",
            }}
            onMouseDown={(e) => e.stopPropagation()}
          >
            <div
              className="flex items-center justify-between gap-3 flex-shrink-0"
              style={{
                padding: '12px 18px',
                borderBottom: '1px solid var(--color-border)',
              }}
            >
              <h2
                id="k2-cheat-sheet-title"
                className="text-[14px] font-semibold text-[var(--color-text-primary)] m-0"
              >
                K2 cheat sheet
              </h2>
              <button
                type="button"
                onClick={close}
                aria-label="Close"
                className="flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
                style={{ borderRadius: 0 }}
              >
                ×
              </button>
            </div>
            <div
              className="overflow-y-auto min-h-0"
              style={{ padding: '14px 18px 18px' }}
            >
              <p className="text-[12.5px] leading-[1.55] text-[var(--color-text-secondary)] m-0 mb-3">
                {K2_CHEAT_SHEET_INTRO}
              </p>
              <div className="mb-3 space-y-2">
                {K2_CHEAT_SHEET_NOTES.map((n) => (
                  <p
                    key={n.note}
                    className="text-[12.5px] leading-[1.5] text-[var(--color-text-secondary)] m-0"
                  >
                    <span className="font-semibold text-[var(--color-text-primary)]">
                      {n.title}
                    </span>
                    {' — '}
                    {n.body}
                  </p>
                ))}
              </div>
              {K2_NOUN_GROUPS.map((group) => (
                <section key={group.title} className="mb-3 last:mb-0">
                  <h3 className="text-[11px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1.5">
                    {group.title}
                  </h3>
                  <ul className="m-0 p-0 list-none">
                    {group.items.map((item) => (
                      <li key={item.noun} className="mb-1.5 last:mb-0">
                        <div className="text-[12.5px] leading-[1.45] text-[var(--color-text-primary)]">
                          <code
                            className="text-[12px]"
                            style={{
                              fontFamily:
                                "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
                              background: 'var(--color-overlay-soft-bg)',
                              padding: '1px 5px',
                            }}
                          >
                            {item.noun}
                          </code>
                          {item.note === 'linux-sidecar' && (
                            <span className="ml-1 text-[10px] uppercase tracking-wider text-[var(--color-text-muted)]">
                              Linux sidecar
                            </span>
                          )}
                          <span className="text-[var(--color-text-secondary)]">
                            {' '}
                            — {item.blurb}
                          </span>
                        </div>
                        {item.example && (
                          <div
                            className="text-[11px] text-[var(--color-text-muted)] mt-0.5"
                            style={{
                              fontFamily:
                                "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
                            }}
                          >
                            {item.example}
                          </div>
                        )}
                      </li>
                    ))}
                  </ul>
                </section>
              ))}
            </div>
          </Surface>
        </>
      )}
    </>
  )
}
