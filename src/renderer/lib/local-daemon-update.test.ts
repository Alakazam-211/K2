import { describe, expect, it } from 'vitest'
import {
  IDLE_DAEMON_UPDATE,
  daemonUpdateCopy,
  isVersionMismatchReason,
  type LocalDaemonUpdateState,
} from './local-daemon-update'

const DROP_IN_MESSAGE =
  'k2-daemon.service starts /home/rosson/dev/K2/target/release/k2-daemon, not /usr/bin/k2-daemon, so restarting it cannot load the v0.46.1 daemon. A drop-in overrides ExecStart: /home/rosson/.config/systemd/user/k2-daemon.service.d/dev-hop.conf. K2 does not edit your systemd files.'

describe('daemonUpdateCopy', () => {
  it('a local version mismatch says Updating K2 daemon, not Connecting', () => {
    const reason = 'version 0.46.0 != app 0.46.1'
    expect(isVersionMismatchReason(reason)).toBe(true)
    const copy = daemonUpdateCopy(reason, IDLE_DAEMON_UPDATE)
    expect(copy).not.toBeNull()
    expect(copy?.heading).toBe('Updating K2 daemon…')
    expect(copy?.forceReload).toBe(false)
  })

  it('other waits keep the plain overlay', () => {
    expect(daemonUpdateCopy('unreachable-or-legacy-daemon', IDLE_DAEMON_UPDATE)).toBeNull()
    expect(daemonUpdateCopy('starting', IDLE_DAEMON_UPDATE)).toBeNull()
    expect(daemonUpdateCopy(null, IDLE_DAEMON_UPDATE)).toBeNull()
    expect(isVersionMismatchReason('remote-unreachable')).toBe(false)
  })

  it('restarting names both versions', () => {
    const state: LocalDaemonUpdateState = {
      phase: 'restarting',
      from_version: '0.46.0',
      to_version: '0.46.1',
      message: null,
    }
    const copy = daemonUpdateCopy('unreachable-or-legacy-daemon', state)
    expect(copy?.heading).toBe('Updating K2 daemon…')
    expect(copy?.subline).toBe('Restarting the daemon (v0.46.0 → v0.46.1).')
  })

  it('a failed restart shows the drop-in message and the reload button', () => {
    const state: LocalDaemonUpdateState = {
      phase: 'failed',
      from_version: '0.46.0',
      to_version: '0.46.1',
      message: DROP_IN_MESSAGE,
    }
    const copy = daemonUpdateCopy('version 0.46.0 != app 0.46.1', state)
    expect(copy?.heading).toBe('K2 daemon needs attention')
    expect(copy?.subline).toBe('The daemon is v0.46.0; this app is v0.46.1.')
    expect(copy?.detail).toBe(DROP_IN_MESSAGE)
    expect(copy?.forceReload).toBe(true)
  })

  it('done falls back to the reason (the gate accepts on its own)', () => {
    const state: LocalDaemonUpdateState = {
      phase: 'done',
      from_version: '0.46.0',
      to_version: '0.46.1',
      message: null,
    }
    expect(daemonUpdateCopy('starting', state)).toBeNull()
  })
})
