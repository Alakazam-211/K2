import { describe, it, expect, beforeEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import {
  isMacTmpPath,
  isRemoteMacTmpPath,
  RemoteMacTmpError,
  throwIfRemoteMacTmp,
} from './remote-mac-tmp'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

function makeRemoteHost(): ConnectHost {
  return {
    id: 'dtl-1',
    label: 'DTL',
    hostname: 'anna.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

describe('isMacTmpPath', () => {
  it('matches /var/folders and /private/var/folders', () => {
    expect(isMacTmpPath('/var/folders/zz/abc/T/NSIRD_screencaptureui_x/Screenshot.png')).toBe(true)
    expect(isMacTmpPath('/private/var/folders/zz/abc/T/Screenshot.png')).toBe(true)
    expect(isMacTmpPath('/var/folders')).toBe(true)
  })

  it('does not refuse /Users or C:\\Users', () => {
    expect(isMacTmpPath('/Users/z3thon/Desktop/Screenshot.png')).toBe(false)
    expect(isMacTmpPath('C:\\Users\\rosson\\Pictures\\x.png')).toBe(false)
    expect(isMacTmpPath('/home/rosson/shot.png')).toBe(false)
    expect(isMacTmpPath('/var/folders-backup/x')).toBe(false)
  })
})

describe('isRemoteMacTmpPath — remote only', () => {
  beforeEach(() => {
    __resetConnectHostStoreForTests()
  })

  it('is false on This Mac even for /var/folders', () => {
    expect(useConnectHostStore.getState().activeHost).toBe('local')
    expect(isRemoteMacTmpPath('/var/folders/zz/abc/T/Screenshot.png')).toBe(false)
  })

  it('is true on a remote host; throwIfRemoteMacTmp does not GET', () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    expect(isRemoteMacTmpPath('/var/folders/zz/abc/T/Screenshot.png')).toBe(true)
    expect(isRemoteMacTmpPath('/Users/z3thon/Desktop/x.png')).toBe(false)
    expect(() => throwIfRemoteMacTmp('/private/var/folders/x/Screenshot.png')).toThrow(RemoteMacTmpError)
  })
})
