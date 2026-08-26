// Mac screenshot / NSIRD jail. Valid on This Mac; never on a Linux DTL.
// Remote-only refuse — do not glob /Users or C:\Users (valid remote homes).

import { useConnectHostStore } from '@/stores/connect-host'

export function isMacTmpPath(path: string): boolean {
  const n = path.replace(/\\/g, '/').toLowerCase()
  return (
    n === '/var/folders' ||
    n.startsWith('/var/folders/') ||
    n === '/private/var/folders' ||
    n.startsWith('/private/var/folders/')
  )
}

export class RemoteMacTmpError extends Error {
  readonly path: string
  constructor(path: string) {
    super('Not available on this server')
    this.name = 'RemoteMacTmpError'
    this.path = path
  }
}

export function isRemoteMacTmpError(err: unknown): boolean {
  return err instanceof Error && err.name === 'RemoteMacTmpError'
}

/** True iff this window's active host is remote AND `path` is Mac tmp. */
export function isRemoteMacTmpPath(path: string): boolean {
  if (useConnectHostStore.getState().activeHost === 'local') return false
  return isMacTmpPath(path)
}

export function throwIfRemoteMacTmp(path: string): void {
  if (isRemoteMacTmpPath(path)) throw new RemoteMacTmpError(path)
}
