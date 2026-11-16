// Policies: what other servers, apps and agents may do on the server you're
// viewing. The right side of Settings → Tunnel, and alone on Connected
// Servers under air-gap. A Member sees a note instead of the switches.

import React from 'react'
import { SettingRow, SettingsGroup } from '../controls/SettingControls'
import { Toggle } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import type { K2Role } from './connect-user-roles'
import { AllowRemoteInstructRow } from '../shared/AllowRemoteInstructRow'
import { DnsManageEnabledRow } from '../shared/DnsManageEnabledRow'
import { AgentsCanCreateConnectionsRow } from '../shared/AgentsCanCreateConnectionsRow'
import { ConnectLoginIngressRow } from '../shared/ConnectLoginIngressRow'
import { BootStatusPublicRow } from '../shared/BootStatusPublicRow'

export function TunnelPolicies({ isRemote, myRole }: { isRemote: boolean; myRole: K2Role | null }): React.JSX.Element {
  // Both switches write to the server this window is on, so on another
  // server they change that server. The public API takes effect per
  // request, so it needs no restart or confirm.
  const federationEnabled = useSettingsStore((s) => s.federationEnabled)
  const setFederationEnabled = useSettingsStore((s) => s.setFederationEnabled)
  const apiEnabled = useSettingsStore((s) => s.apiEnabled)
  const setApiEnabled = useSettingsStore((s) => s.setApiEnabled)
  return (
    <SettingsGroup
      title="Policies"
      hint={`What other servers, apps and agents may do on ${isRemote ? 'the server you are viewing' : 'this server'}.`}
      card
      testId="tunnel-policies"
    >
      {myRole !== 'member' ? (
        <>
          <SettingRow
            settingId="k2-connect.federation"
            label="Connect agents across servers"
            hint="Agents here can message agents on the servers you add under Connected Servers."
          >
            <Toggle
              checked={federationEnabled}
              onChange={(next) => void setFederationEnabled(next)}
              aria-label="Connect agents across servers"
            />
          </SettingRow>
          <AllowRemoteInstructRow />
          <SettingRow
            settingId="k2-connect.public-api"
            label="Public API"
            hint="Apps with a key from Settings → API Keys can use this server. Takes effect right away."
          >
            <Toggle checked={apiEnabled} onChange={(next) => void setApiEnabled(next)} aria-label="Public API" />
          </SettingRow>
          <DnsManageEnabledRow />
          <AgentsCanCreateConnectionsRow />
          <ConnectLoginIngressRow />
          <BootStatusPublicRow />
        </>
      ) : (
        <SettingRow label="Only an admin can change these." hint="Ask the owner of this server if something here needs to change.">
          {null}
        </SettingRow>
      )}
    </SettingsGroup>
  )
}
