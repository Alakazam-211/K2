// Advanced on Settings → Tunnel (its right side, under Policies): set the
// tunnel up by hand (address, token, relay and port), or release or forget
// it on this computer. The page owns the state and the calls; this draws
// the rows. A field drops under its label when the side is too narrow for
// both on one line, like every other Settings row.

import React from 'react'
import {
  ROW_WRAP,
  SETTING_FIELD_WIDTH,
  SETTING_ROW_LABEL,
  SETTING_ROW_LABEL_MIN,
  SaveError,
  SaveNote,
  SettingRow,
  SettingsGroup,
  type SaveState,
} from '../controls/SettingControls'
import { Button, Input } from '@/components/ui'
import { RELAY_HOST, ZONE, ZONE_SUFFIX } from '@/lib/hosts'

// No relay address lives here: the daemon owns the default and the relay
// list. The form shows what the daemon stores, and only placeholders
// otherwise (an empty field is never sent).
const RELAY_PLACEHOLDER = RELAY_HOST
const PORT_PLACEHOLDER = '7000'

/** The by-hand form's values and what changing them does. */
export interface TunnelManualForm {
  subdomain: string
  onSubdomain: (v: string) => void
  token: string
  onToken: (v: string) => void
  /** A token is already saved (the daemon never sends it back). */
  tokenSet: boolean
  serverAddr: string
  onServerAddr: (v: string) => void
  serverPort: string
  onServerPort: (v: string) => void
  saveState: SaveState
  busy: boolean
  onSave: () => void
}

const FIELD_ROW = `${ROW_WRAP} items-center`

function ManualRows({ form }: { form: TunnelManualForm }): React.JSX.Element {
  return (
    <div className="space-y-3" data-testid="tunnel-manual">
      <div className={FIELD_ROW} data-settings-id="k2-connect.subdomain">
        <label htmlFor="tunnel-manual-sub" className={`${SETTING_ROW_LABEL_MIN} ${SETTING_ROW_LABEL}`}>Address</label>
        <div className={`${SETTING_FIELD_WIDTH} max-w-full flex items-center gap-1.5 flex-shrink-0`}>
          <Input id="tunnel-manual-sub" className="flex-1 min-w-0 type-control" placeholder="alex" value={form.subdomain} onChange={(e) => form.onSubdomain(e.target.value)} />
          <span className="type-hint text-[var(--color-text-secondary)]">{ZONE_SUFFIX}</span>
        </div>
      </div>
      <div className={FIELD_ROW} data-settings-id="k2-connect.token">
        <label htmlFor="tunnel-manual-token" className={`${SETTING_ROW_LABEL_MIN} ${SETTING_ROW_LABEL}`}>
          Token{form.tokenSet && <span className="type-hint text-[var(--color-text-secondary)] ml-2">saved</span>}
        </label>
        <Input
          id="tunnel-manual-token"
          className={`${SETTING_FIELD_WIDTH} max-w-full flex-shrink-0 type-control`}
          type="password"
          placeholder={form.tokenSet ? 'Leave blank to keep the saved one' : `From your ${ZONE} account`}
          value={form.token}
          onChange={(e) => form.onToken(e.target.value)}
        />
      </div>
      <div className={FIELD_ROW} data-settings-id="k2-connect.server-addr">
        <label htmlFor="tunnel-manual-relay" className={`${SETTING_ROW_LABEL_MIN} ${SETTING_ROW_LABEL}`}>Relay and port</label>
        <div className={`${SETTING_FIELD_WIDTH} max-w-full flex items-center gap-1.5 flex-shrink-0`}>
          <Input
            id="tunnel-manual-relay"
            className="flex-1 min-w-0 type-control"
            placeholder={RELAY_PLACEHOLDER}
            value={form.serverAddr}
            onChange={(e) => form.onServerAddr(e.target.value)}
          />
          <Input
            aria-label="Relay port"
            className="w-16 flex-shrink-0 type-control"
            placeholder={PORT_PLACEHOLDER}
            value={form.serverPort}
            onChange={(e) => form.onServerPort(e.target.value)}
          />
        </div>
      </div>
      <SaveError state={form.saveState} />
      <div className="flex items-center justify-end gap-2">
        <SaveNote state={form.saveState} />
        <Button onClick={form.onSave} disabled={form.busy}>
          {form.busy ? 'Saving…' : 'Save'}
        </Button>
      </div>
    </div>
  )
}

export function TunnelAdvanced({
  showManual,
  onToggleManual,
  form,
  releaseHost,
  onRelease,
  onForget,
  disabled,
}: {
  showManual: boolean
  onToggleManual: () => void
  form: TunnelManualForm
  /** The address Release names, or null when there is nothing to release
   *  or forget (no token saved, or already released). */
  releaseHost: string | null
  onRelease: () => void
  onForget: () => void
  /** Another tunnel change is under way. */
  disabled: boolean
}): React.JSX.Element {
  return (
    <SettingsGroup
      title="Advanced"
      hint="Set the tunnel up by hand, or remove it from this computer."
      card
      testId="tunnel-advanced"
    >
      <SettingRow
        label="Set up by hand"
        hint="Enter an address and token yourself instead of signing in."
        below={showManual ? <ManualRows form={form} /> : undefined}
      >
        <Button variant="quiet" aria-expanded={showManual} onClick={onToggleManual}>
          {showManual ? 'Hide' : 'Set up by hand'}
        </Button>
      </SettingRow>
      {releaseHost !== null && (
        <SettingRow
          settingId="k2-connect.release-device"
          label={`Release ${releaseHost} from this computer`}
          hint="It stops reaching this computer. Your account keeps the address for another computer."
        >
          <Button variant="danger-quiet" onClick={onRelease} disabled={disabled}>
            Release…
          </Button>
        </SettingRow>
      )}
      {releaseHost !== null && (
        <SettingRow
          label="Forget the tunnel settings"
          hint={`Deletes this computer's tunnel settings without telling ${ZONE}. For settings copied from another computer.`}
        >
          <Button variant="danger-quiet" onClick={onForget} disabled={disabled}>
            Forget…
          </Button>
        </SettingRow>
      )}
    </SettingsGroup>
  )
}
