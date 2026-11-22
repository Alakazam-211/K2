// Settings → Language Models → Subs / Tokens: the per-provider auto-rotate
// switch, next to "Use next token". On, the daemon presses that same Next by
// itself when the server default subscription reaches 99% of its limit
// (headless: the loop lives in the daemon, not here). Off by default, and a
// person's own decision (it may go against the provider's terms), so turning
// it on asks first and then confirms it's them (Touch ID or their login
// where the server offers it, else the vault password). Turning it off is
// one click. Copy is short on purpose; K2 Design polishes it later.

import React from 'react'
import { useId, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import {
  AUTO_NEXT_NOTE,
  AutoNextPasswordNeeded,
  autoNextOff,
  autoNextOn,
  errorText,
  type LlmTool,
} from '@/stores/llm-accounts'
import { Button, Input, Toggle } from '@/components/ui'
import { SettingRow } from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, DIALOG_MUTED, Field } from '../sections/AccessDialogs'
import { PRODUCT_NAME } from '@/lib/brand'

/** Who turned it on, in words. */
export function autoNextByText(by: string | null): string | null {
  if (!by) return null
  if (by === 'owner-token') return 'the owner'
  return by.startsWith('login:') ? by.slice('login:'.length) : by
}

export function AutoNextRow({
  scope,
  tool,
  canChange,
  changeReason,
}: {
  scope: ServerScope
  tool: LlmTool
  canChange: boolean
  changeReason: string | null
}): React.JSX.Element {
  const on = tool.autoNext?.on === true
  const [asking, setAsking] = useState(false)
  const [needPassword, setNeedPassword] = useState(false)
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [problem, setProblem] = useState<string | null>(null)
  const pwId = useId()

  const close = (): void => {
    setAsking(false)
    setNeedPassword(false)
    setPassword('')
    setProblem(null)
  }

  const turnOn = async (): Promise<void> => {
    setBusy(true)
    setProblem(null)
    try {
      await autoNextOn(scope, tool.tool, needPassword ? password : undefined)
      close()
    } catch (e) {
      if (e instanceof AutoNextPasswordNeeded) setNeedPassword(true)
      else setProblem(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const turnOff = async (): Promise<void> => {
    setBusy(true)
    setProblem(null)
    try {
      await autoNextOff(scope, tool.tool)
    } catch (e) {
      setProblem(errorText(e))
    } finally {
      setBusy(false)
    }
  }

  const by = autoNextByText(tool.autoNext?.by ?? null)
  const hint = [
    on ? `On${by ? `, turned on by ${by}` : ''}: at 99% ${PRODUCT_NAME} moves to the next ${tool.display} subscription by itself.` : `Off: ${PRODUCT_NAME} never switches on its own.`,
    tool.autoNext?.lastLine ?? null,
  ]
    .filter(Boolean)
    .join(' ')

  return (
    <>
      <SettingRow
        label="Switch automatically at the limit"
        hint={hint}
        below={!asking && problem ? <p className="type-hint text-[var(--color-status-error-soft)]">{problem}</p> : undefined}
        testId={`llm-auto-next-${tool.tool}`}
      >
        <Toggle
          checked={on}
          disabled={busy || !canChange}
          title={canChange ? undefined : (changeReason ?? undefined)}
          aria-label={`Switch ${tool.display} automatically at the limit`}
          onChange={(next) => {
            if (next) setAsking(true)
            else void turnOff()
          }}
        />
      </SettingRow>
      {asking ? (
        <AccessSheet
          title={`Switch ${tool.display} automatically?`}
          onClose={close}
          busy={busy}
          testId="llm-auto-next-confirm"
          footer={
            <>
              <Button variant="ghost" size="md" onClick={close} disabled={busy} autoFocus>
                Cancel
              </Button>
              <Button
                variant="accent"
                size="md"
                disabled={busy || (needPassword && password === '')}
                onClick={() => void turnOn()}
              >
                Turn on
              </Button>
            </>
          }
        >
          <div className="space-y-3">
            <p className={DIALOG_MUTED}>{AUTO_NEXT_NOTE}</p>
            <p className={DIALOG_MUTED}>You confirm it’s you next. Agents can’t turn this on.</p>
            {needPassword ? (
              <Field label="Vault password" htmlFor={pwId} hint="This server confirms with its vault password." problem={problem}>
                <Input
                  id={pwId}
                  type="password"
                  className={DIALOG_FIELD}
                  value={password}
                  autoComplete="off"
                  onChange={(e) => setPassword(e.target.value)}
                />
              </Field>
            ) : problem ? (
              <p className="type-hint text-[var(--color-status-error-soft)]">{problem}</p>
            ) : null}
          </div>
        </AccessSheet>
      ) : null}
    </>
  )
}
