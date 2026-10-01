// A `ServerScope` for room tests (Home M3). It is a plain pinned scope for
// `hostKey`: no saved host, no creds resolution of its own. Tests that mock
// `@/lib/daemon-cli` / `@/kessel/daemon-ws` key their fake daemons on
// `scope.id` or `scope.hostKey`, so two of these never share a request
// target. `creds()` throws: a test that reaches real transport with a fake
// scope has a mock missing, and should fail loudly.

import type { ServerScope } from '@/kessel/server-scope'

export function fakeScope(hostKey: string, opts?: { remote?: boolean }): ServerScope {
  const fail = (): never => {
    throw new Error(`fakeScope(${hostKey}): real transport reached; mock the request layer`)
  }
  return {
    id: `host:${hostKey}`,
    isPrimary: false,
    hostKey,
    connectionKey: `host:${hostKey}`,
    isRemote: opts?.remote ?? true,
    label: hostKey,
    connectHost: () => null,
    isWindowHost: () => false,
    creds: async () => fail(),
    httpBase: async () => fail(),
    wsBase: async () => fail(),
    serverSupports: () => true,
  }
}
