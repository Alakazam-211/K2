import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import { Button, DialogFrame, DialogScrim } from '@/components/ui'
import { SettingDropdown } from '@/components/Settings/controls/SettingControls'
import { type ComposeStatus } from '@/components/Terminal/terminalCompose'

// Per-provider resume contract. Shared by Chat history and the pinned
// chat so both dialogs list the same harnesses. Resume flags stay here;
// the pinned spawn does not build argv from them.
interface ProviderConfig {
  command: string
  label: string
  resumeFlag?: string
  resumeSubcommand?: string
}

export const PROVIDER_CONFIG: Record<string, ProviderConfig> = {
  claude: { command: 'claude', label: 'Claude', resumeFlag: '--resume' },
  cursor: { command: 'cursor-agent', label: 'Cursor', resumeFlag: '--resume' },
  grok: { command: 'grok', label: 'Grok', resumeFlag: '--resume' },
  gemini: { command: 'gemini', label: 'Gemini', resumeFlag: '--resume' },
  pi: { command: 'pi', label: 'Pi', resumeFlag: '--session' },
  codex: { command: 'codex', label: 'Codex', resumeSubcommand: 'resume' },
  hermes: { command: 'hermes', label: 'Hermes', resumeFlag: '--resume' },
}

export function providerKeyForCommand(command: string | null | undefined): string | null {
  const base = command?.trim().split(/\s+/)[0]?.split(/[/\\]/).pop() ?? ''
  if (!base) return null
  for (const [key, cfg] of Object.entries(PROVIDER_CONFIG)) {
    if (cfg.command === base) return key
  }
  return null
}

export type ContinueMode = 'recent' | 'full'

export interface ContinueNewChatSource {
  provider: string
  sessionId: string
  projectPath: string
  displayName: string
}

export interface ContinueSpawnRequest {
  text: string
  targetProvider: string
  mode: ContinueMode
}

export function continueSendError(status: ComposeStatus): string {
  switch (status.kind) {
    case 'pty_died':
      return status.hint?.trim() || 'The terminal exited before the message was sent.'
    case 'pty_stalled':
      return status.hint?.trim() || 'The terminal stalled before the message was sent.'
    case 'busy':
      return [status.reason, status.hint].filter((part) => !!part).join(': ')
        || 'The message was not delivered.'
    case 'error':
      return status.message
    default:
      return 'The message was not delivered.'
  }
}

async function copyText(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text)
  } catch {
    const ta = document.createElement('textarea')
    ta.value = text
    document.body.appendChild(ta)
    ta.select()
    document.execCommand('copy')
    document.body.removeChild(ta)
  }
}

/**
 * Shared continue dialog. Posts the seed, then the caller decides what
 * to do with the text. History opens a strip tab. The pin force-respawns
 * in place. Esc, Cancel, and the scrim only close.
 */
export function ContinueNewChatDialog({
  source,
  onClose,
  onSpawn,
}: {
  source: ContinueNewChatSource
  onClose: () => void
  onSpawn: (request: ContinueSpawnRequest, stillOpen: () => boolean) => Promise<void>
}): React.JSX.Element {
  const [target, setTarget] = useState(
    PROVIDER_CONFIG[source.provider] ? source.provider : 'claude',
  )
  const [mode, setMode] = useState<ContinueMode>('recent')
  const [error, setError] = useState<string | null>(null)
  const [seed, setSeed] = useState<string | null>(null)
  const [inFlight, setInFlight] = useState(false)
  const genRef = useRef(0)

  const requestClose = useCallback(() => {
    genRef.current += 1
    onClose()
  }, [onClose])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      requestClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [requestClose])

  const start = useCallback(async () => {
    if (inFlight) return
    const gen = genRef.current
    const stillOpen = (): boolean => genRef.current === gen
    setInFlight(true)
    setError(null)
    try {
      const seeded = await daemonCliPost<{ text?: unknown }>('chat/continue-seed', {
        provider: source.provider,
        sessionId: source.sessionId,
        projectPath: source.projectPath,
        mode,
        targetProvider: target,
      })
      if (!stillOpen()) return
      if (!seeded || typeof seeded.text !== 'string' || seeded.text.length === 0) {
        throw new Error('continue-seed returned no text')
      }
      const text = seeded.text
      setSeed(text)
      await onSpawn({ text, targetProvider: target, mode }, stillOpen)
      if (!stillOpen()) return
      requestClose()
    } catch (err) {
      if (!stillOpen()) return
      setError(err instanceof Error ? err.message : String(err))
      setInFlight(false)
    }
  }, [inFlight, mode, onSpawn, requestClose, source.projectPath, source.provider, source.sessionId, target])

  const sourceLabel = PROVIDER_CONFIG[source.provider]?.label ?? source.provider

  return (
    <>
      <DialogScrim
        onMouseDown={(e) => {
          e.stopPropagation()
          requestClose()
        }}
      />
      <DialogFrame
        data-testid="continue-new-chat"
        style={{
          minWidth: 340,
          maxWidth: 440,
          padding: '20px 24px',
          fontFamily: 'var(--font-mono, monospace)',
        }}
      >
        <div style={{ fontSize: 14, fontWeight: 600, color: 'var(--color-text-primary)', marginBottom: 8 }}>
          Continue in a new chat
        </div>
        <div style={{ fontSize: 12, color: 'var(--color-text-primary)', marginBottom: 2 }}>
          {source.displayName}
        </div>
        <div style={{ fontSize: 11, color: 'var(--color-text-muted)', marginBottom: 14 }}>
          From {sourceLabel}
        </div>
        <div style={{ marginBottom: 12 }}>
          <div style={{ fontSize: 11, color: 'var(--color-text-secondary)' }}>Harness</div>
          <SettingDropdown
            ariaLabel="Harness"
            fullWidth
            menuAlign="left"
            className="mt-1"
            disabled={inFlight}
            value={target}
            onChange={setTarget}
            options={Object.entries(PROVIDER_CONFIG).map(([key, cfg]) => ({ value: key, label: cfg.label }))}
          />
        </div>
        <div style={{ fontSize: 11, color: 'var(--color-text-secondary)', marginBottom: 4 }}>Context</div>
        <label style={{ display: 'block', fontSize: 12, color: 'var(--color-text-primary)', marginBottom: 2 }}>
          <input
            type="radio"
            name="continue-context"
            checked={mode === 'recent'}
            disabled={inFlight}
            onChange={() => setMode('recent')}
          />{' '}
          Recent turns
        </label>
        <div style={{ fontSize: 10, color: 'var(--color-text-muted)', margin: '0 0 8px 18px' }}>
          Last user request and last assistant reply.
        </div>
        <label style={{ display: 'block', fontSize: 12, color: 'var(--color-text-primary)', marginBottom: 2 }}>
          <input
            type="radio"
            name="continue-context"
            checked={mode === 'full'}
            disabled={inFlight}
            onChange={() => setMode('full')}
          />{' '}
          Full history
        </label>
        <div style={{ fontSize: 10, color: 'var(--color-text-muted)', margin: '0 0 14px 18px' }}>
          The saved history. It uses more context.
        </div>
        {error && (
          <div role="alert" style={{ fontSize: 12, color: 'var(--color-danger)', marginBottom: 12, whiteSpace: 'pre-wrap' }}>
            {error}
          </div>
        )}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8 }}>
          {seed !== null && (
            <Button
              type="button"
              variant="ghost"
              size="md"
              onClick={() => { void copyText(seed) }}
            >
              Copy text
            </Button>
          )}
          <Button type="button" variant="ghost" size="md" onClick={requestClose}>
            Cancel
          </Button>
          <Button
            type="button"
            variant="accent"
            size="md"
            disabled={inFlight}
            onClick={() => { void start() }}
          >
            Start
          </Button>
        </div>
      </DialogFrame>
    </>
  )
}
