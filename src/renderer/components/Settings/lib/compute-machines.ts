// compute-machines.ts — "Machines you can use" rows (Settings → Compute).
//
// Two truths merged, never invented: this server's nodes (the daemon,
// `/cli/compute/nodes`) and the account's machines that offer compute (K2
// Connect, `compute_machines`). A node enrolled by code shows even when it
// isn't on the account; an account machine shows even when it isn't a
// node here yet. Matched by the node key fingerprint.

import type { ComputeNode } from '../sections/compute-api'
import { machineOnline, type ComputeMachine } from './compute-account'

export type MachineStatus =
  /** A node of this server, taking jobs. */
  | 'on'
  /** Enrolled, waiting for the owner's OK (the 6-digit code). */
  | 'pending'
  /** Switched off: finishing its jobs, then removed. */
  | 'retiring'
  /** Asked to join a moment ago; it checks in shortly. */
  | 'requested'
  /** Offers compute, not a node of this server. */
  | 'off'
  /** A node of another server. */
  | 'elsewhere'

export interface MachineRow {
  key: string
  name: string
  /** The machine's k2.dev label, when it is on the account. */
  sub: string | null
  os: string | null
  arch: string | null
  cpuCores: number | null
  memBytes: number | null
  diskFreeBytes: number | null
  online: boolean
  status: MachineStatus
  /** Host of the other server (`elsewhere`). */
  elsewhere: string | null
  node: ComputeNode | null
  machine: ComputeMachine | null
}

/** How long a "requested" mark lasts without the node showing up. */
export const REQUEST_TTL_MS = 3 * 60 * 1000

const ENROLLED = new Set(['pending', 'connecting', 'online', 'offline'])

function nodeStatus(n: ComputeNode): MachineStatus {
  if (n.state === 'pending') return 'pending'
  if (n.retiring) return 'retiring'
  return 'on'
}

/** This server's k2.dev label (`rosson.k2.dev` → `rosson`), else null. */
export function controllerSub(controller: string | null | undefined): string | null {
  const host = (controller ?? '').trim().toLowerCase().replace(/^[a-z]+:\/\//, '').replace(/[/:].*$/, '')
  const m = /^([a-z0-9-]+)\.k2\.dev$/.exec(host)
  return m ? m[1] : null
}

/** A node name from a machine (its own name, else its label). */
export function nodeNameFor(m: Pick<ComputeMachine, 'node_name' | 'sub'>): string {
  const raw = (m.node_name || m.sub || 'node').toLowerCase()
  const s = raw.replace(/[^a-z0-9-]/g, '-').replace(/^-+/, '').slice(0, 32).replace(/-+$/, '')
  return s || 'node'
}

export function mergeMachines(
  nodes: ComputeNode[],
  machines: ComputeMachine[],
  controllerHost: string,
  requested: Record<string, number> = {},
  nowMs: number = Date.now(),
): MachineRow[] {
  const byFp = new Map(machines.map((m) => [m.node_fp.toLowerCase(), m]))
  const used = new Set<string>()
  const rows: MachineRow[] = []
  for (const n of nodes) {
    if (n.state === 'revoked') continue
    const m = byFp.get(n.fingerprint.toLowerCase()) ?? null
    if (m) used.add(m.node_fp.toLowerCase())
    const offer = n.offer
    rows.push({
      key: `node:${n.id}`,
      name: n.name,
      sub: m?.sub ?? null,
      os: n.os ?? m?.os ?? null,
      arch: n.arch ?? m?.arch ?? null,
      cpuCores: offer ? offer.caps.cpu_millis / 1000 : (m?.cpu_cores ?? null),
      memBytes: offer ? offer.caps.mem_bytes : (m?.mem_bytes ?? null),
      diskFreeBytes: offer ? offer.free.disk_free_bytes : (m?.disk_free_bytes ?? null),
      online: n.online,
      status: nodeStatus(n),
      elsewhere: null,
      node: n,
      machine: m,
    })
  }
  const host = controllerHost.trim().toLowerCase()
  for (const m of machines) {
    if (used.has(m.node_fp.toLowerCase())) continue
    const ctl = (m.controller ?? '').trim().toLowerCase()
    const enrolledElsewhere = !!ctl && ctl !== host && ENROLLED.has(m.node_state ?? '')
    const asked = requested[m.sub]
    let status: MachineStatus = 'off'
    if (enrolledElsewhere) status = 'elsewhere'
    else if (asked !== undefined && nowMs - asked < REQUEST_TTL_MS) status = 'requested'
    rows.push({
      key: `machine:${m.sub}:${m.node_fp}`,
      name: m.node_name || m.sub,
      sub: m.sub,
      os: m.os,
      arch: m.arch,
      cpuCores: m.cpu_cores,
      memBytes: m.mem_bytes,
      diskFreeBytes: m.disk_free_bytes,
      online: machineOnline(m, nowMs),
      status,
      elsewhere: enrolledElsewhere ? ctl : null,
      node: null,
      machine: m,
    })
  }
  return rows.sort((a, b) => a.name.localeCompare(b.name))
}

/** The pending code the machine reports matches what this server computed. */
export function sasAgrees(node: ComputeNode | null, machine: ComputeMachine | null): boolean {
  const a = (node?.sas ?? '').replace(/\s+/g, '')
  const b = (machine?.sas ?? '').replace(/\s+/g, '')
  return /^\d{6}$/.test(a) && a === b
}
