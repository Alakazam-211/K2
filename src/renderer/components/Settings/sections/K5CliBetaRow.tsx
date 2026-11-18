import React, { useEffect, useState } from 'react'

import { BetaTag, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '../controls/SettingControls'
import { ActionNote, useActionNote } from './ActionNote'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { CLI_NAME } from '@/lib/brand'
import type { ServerScope } from '@/kessel/server-scope'

// Settings → General → Server, its own "Advanced" card: "New k5 command (Beta)". The
// owner's switch on the FOCUSED server (`GET`/`POST /cli/k5-cli`): on, the
// k2 and k5 commands there run the new k5 program instead of the shell
// script, where that server's release has it. The daemon owns the switch
// and re-points the commands; this row only shows and flips it. Off by
// default. The route answers the owner only, so anyone else (and a server
// from before the switch) sees no card.

/** The switch's words; it carries a Beta tag on screen. */
export const K5_CLI_NAME = 'New k5 command'
/** The accessible name: the words plus "(Beta)", which the tag says on screen. */
export const K5_CLI_LABEL = `${K5_CLI_NAME} (Beta)`
export const K5_CLI_LINE = `On: the ${CLI_NAME} and k5 commands run the new k5 program instead of the shell script, where this server's release has it. Off by default: the shell script, as before. Not on Windows yet.`

/** `GET /cli/k5-cli`. `runs` and `note` come with the links' choice: what
 *  the commands run now, and why the script when the switch is on. */
export interface K5CliState {
  on: boolean
  runs?: 'script' | 'binary'
  note?: string | null
}

/** The answer, read strictly: anything else is an error, never "off". */
export function readK5CliState(v: unknown): K5CliState {
  const o = v as Record<string, unknown> | null
  if (!o || typeof o !== 'object' || typeof o.on !== 'boolean') {
    throw new Error("the server's answer has no on/off for the k5 command")
  }
  const runs = o.runs === 'script' || o.runs === 'binary' ? o.runs : undefined
  const note = typeof o.note === 'string' ? o.note : null
  return { on: o.on, runs, note }
}

/** One line under the switch: what the commands run now. */
export function k5CliStatusLine(s: K5CliState): string | null {
  if (!s.runs) return null
  if (s.runs === 'binary') return `The ${CLI_NAME} and k5 commands run the k5 program.`
  if (s.on && s.note) return `The ${CLI_NAME} command still runs the shell script: ${s.note}.`
  return null
}

export function K5CliBetaRow({ scope }: { scope: ServerScope }): React.JSX.Element | null {
  const [state, setState] = useState<K5CliState | null>(null)
  const [busy, setBusy] = useState(false)
  const { note, done, failed, clear } = useActionNote()

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const s = readK5CliState(await daemonCliGet<unknown>(scope, 'k5-cli'))
        if (!cancelled) setState(s)
      } catch {
        // Not the owner, or a server from before the switch: no card.
        if (!cancelled) setState(null)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [scope])

  if (!state) return null

  const flip = async (next: boolean): Promise<void> => {
    if (busy) return
    clear()
    setBusy(true)
    try {
      const s = readK5CliState(await daemonCliPost<unknown>(scope, 'k5-cli', { on: next }))
      setState(s)
      done(s.on ? 'On.' : 'Off. The commands run the shell script.')
    } catch (e) {
      failed(next ? 'Couldn’t turn the new k5 command on.' : 'Couldn’t turn the new k5 command off.', e)
    } finally {
      setBusy(false)
    }
  }

  const status = k5CliStatusLine(state)
  return (
    <SettingsGroup title="Advanced" card testId="k5-cli-card">
      <SettingSwitchRow
        settingId="general.k5-cli-beta"
        control={
          <Toggle
            checked={state.on}
            onChange={(v: boolean) => void flip(v)}
            disabled={busy}
            aria-label={K5_CLI_LABEL}
            aria-busy={busy || undefined}
          />
        }
      >
        <div className="type-row text-[var(--color-text-primary)] flex items-center gap-2">
          <span>{K5_CLI_NAME}</span>
          <BetaTag />
        </div>
        <div className={SETTING_ROW_HINT}>{K5_CLI_LINE}</div>
        {status ? (
          <div className={SETTING_ROW_HINT} data-testid="k5-cli-status">
            {status}
          </div>
        ) : null}
        {note ? (
          <div className="mt-2">
            <ActionNote note={note} />
          </div>
        ) : null}
      </SettingSwitchRow>
    </SettingsGroup>
  )
}
