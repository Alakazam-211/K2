// Garden data — one typed call per route Settings uses
// (contract-garden-data-v1 §4.3, §4.5). Written in C0; read-only for
// B1/B2/B3 (changes go through the integrator).
//
// Garden data belongs to this computer, so every call goes to
// `zenLocalScope()` (the owner token), never `primaryScope()`. Widget calls
// don't use this file: they go through `/cli/zen/call` in
// `lib/zen/zen-custom-store.ts` (B2). Errors throw like every daemon call;
// the envelope is `GardenErrorEnvelope`.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope } from '@/lib/zen/zen-api'
import type {
  GardenAdminBody,
  GardenAdminRevokeBody,
  GardenAdminRevokeResponse,
  GardenCallBody,
  GardenCallResponse,
  GardenExportResponse,
  GardenGrantBody,
  GardenGrantResponse,
  GardenGrantsResponse,
  GardenImportBody,
  GardenImportResponse,
  GardenRevokeBody,
  GardenRevokeResponse,
  GardenShowResponse,
  GardenSpaceDeleteResponse,
  GardenSpaceMoveResponse,
  GardenSpacesResponse,
} from './garden-data-types'

// ── the person's views (Settings → Gardens → Local Data) ──────────

export function listGardenSpaces(): Promise<GardenSpacesResponse> {
  return daemonCliGet<GardenSpacesResponse>(zenLocalScope(), 'zen/data/spaces')
}

export function showGardenSpace(widget: string): Promise<GardenShowResponse> {
  return daemonCliGet<GardenShowResponse>(zenLocalScope(), 'zen/data/show', { widget })
}

export function listGardenGrants(widget: string): Promise<GardenGrantsResponse> {
  return daemonCliGet<GardenGrantsResponse>(zenLocalScope(), 'zen/data/grants', { widget })
}

/** One export page (`at` = `YYYY-MM-DD` reads a snapshot). */
export function exportGardenData(widget: string, opts: { at?: string; after?: string } = {}): Promise<GardenExportResponse> {
  return daemonCliGet<GardenExportResponse>(zenLocalScope(), 'zen/data/export', {
    widget,
    at: opts.at,
    after: opts.after,
  })
}

/** One page of a widget's files with what they count for (GB17). */
export function listGardenBlobs(
  widget: string,
  opts: { after?: string; limit?: number } = {},
): Promise<{ items: Array<{ ref: string; size: number; mime: string; name: string | null; refs: number; pending: boolean }>; next: string | null; bytes: number; quota: number }> {
  return daemonCliGet(zenLocalScope(), 'zen/data/blob/list', { widget, after: opts.after, limit: opts.limit })
}

/** One ranged read of a widget's file (`len` ≤ 8 MiB), as the person. */
export function readGardenBlob(widget: string, ref: string, offset: number, len: number): Promise<{ base64: string; len: number; eof: boolean; size: number }> {
  return daemonCliGet(zenLocalScope(), 'zen/data/blob/get', { widget, ref, offset, len })
}

/** One history item (`store.history`, prd-garden-board-needs-v1 §6.2). */
export interface GardenHistoryItem {
  key: string
  rev: number
  deleted: boolean
  value: unknown
  prev: unknown
  prevKind: 'value' | 'none' | 'unknown'
  by: string
  at: number
  restore?: string
}

/** One page of every change kept (7 days), newest first. */
export function gardenHistory(
  widget: string,
  opts: { prefix?: string; key?: string; before?: number; after?: string; limit?: number } = {},
): Promise<{ head: number; oldest: number; items: GardenHistoryItem[]; next: { before: number; after?: string } | null }> {
  return daemonCliGet(zenLocalScope(), 'zen/data/history', { widget, ...opts })
}

/** What a restore wrote (or, with `dryRun`, would write). */
export interface GardenRestored {
  id: string | null
  rev: number | null
  written: number
  deleted: number
  families: string[]
  keys: string[]
}

/** The person's restore (any key set; the whole widget too). */
export function restoreGardenData(body: {
  widget: string
  toRev?: number
  at?: number
  snapshot?: string
  prefixes?: string[]
  except?: string[]
  dryRun?: boolean
}): Promise<GardenRestored> {
  return daemonCliPost<GardenRestored>(zenLocalScope(), 'zen/data/restore', body)
}

// ── the person's changes ─────────────────────────────────────────────

export function grantGardenData(body: GardenGrantBody): Promise<GardenGrantResponse> {
  return daemonCliPost<GardenGrantResponse>(zenLocalScope(), 'zen/data/grant', body)
}

export function revokeGardenData(body: GardenRevokeBody): Promise<GardenRevokeResponse> {
  return daemonCliPost<GardenRevokeResponse>(zenLocalScope(), 'zen/data/revoke', body)
}

export function makeGardenAdmin(body: GardenAdminBody): Promise<GardenGrantResponse> {
  return daemonCliPost<GardenGrantResponse>(zenLocalScope(), 'zen/data/admin', body)
}

/** End a delegation; `dryRun` answers the cascade count without changing anything. */
export function revokeGardenAdmin(body: GardenAdminRevokeBody): Promise<GardenAdminRevokeResponse> {
  return daemonCliPost<GardenAdminRevokeResponse>(zenLocalScope(), 'zen/data/admin/revoke', body)
}

/** Delete a widget's data (`confirm` must equal the widget name). */
export function deleteGardenSpace(widget: string, confirm: string): Promise<GardenSpaceDeleteResponse> {
  return daemonCliPost<GardenSpaceDeleteResponse>(zenLocalScope(), 'zen/data/space/delete', { widget, confirm })
}

export function moveGardenSpace(from: string, to: string): Promise<GardenSpaceMoveResponse> {
  return daemonCliPost<GardenSpaceMoveResponse>(zenLocalScope(), 'zen/data/space/move', { from, to })
}

export function importGardenData(body: GardenImportBody): Promise<GardenImportResponse> {
  return daemonCliPost<GardenImportResponse>(zenLocalScope(), 'zen/data/import', body)
}

/** "It's mine": stamp the live version of a widget as the person's.
 *  prd-garden-access-v1 §2: retired with Garden access on (the daemon
 *  answers 410 `retired`; Widget data no longer offers it; Sign in a
 *  history modal covers the case). */
export function claimWidget(widget: string): Promise<{ widget: string; hash: string }> {
  return daemonCliPost<{ widget: string; hash: string }>(zenLocalScope(), 'zen/widget/claim', { widget })
}

// ── the widget door (B2's bridge uses this) ──────────────────────────

export function gardenCall<T = unknown>(body: GardenCallBody): Promise<GardenCallResponse<T>> {
  return daemonCliPost<GardenCallResponse<T>>(zenLocalScope(), 'zen/call', body)
}

/** Where a widget door byte call comes from: the placement and the hash
 *  its frame runs (prd-garden-board-needs-v1 GB13). */
export interface GardenBlobPlace {
  garden: string
  placement: string
  hash: string
}

/** One chunk of a blob upload (`/cli/zen/blob/put`, ≤ 8 MiB raw). */
export function gardenBlobPut(place: GardenBlobPlace, chunk: Record<string, unknown>): Promise<unknown> {
  return daemonCliPost<unknown>(zenLocalScope(), 'zen/blob/put', { ...place, ...chunk })
}

/** Files K2's own dialog picked, or a drop, by path, into the widget's
 *  data (`/cli/zen/blob/ingest`, 0.46.6 GP3/GP4). Never a path back. */
export function gardenBlobIngest(
  place: GardenBlobPlace,
  paths: string[],
  accept: string[],
): Promise<{ files: { ref: string; name: string; size: number; mime: string }[] }> {
  return daemonCliPost(zenLocalScope(), 'zen/blob/ingest', { ...place, paths, accept })
}

/** One ranged read of a blob (`/cli/zen/blob/get`). */
export function gardenBlobGet(place: GardenBlobPlace, params: Record<string, string | number>): Promise<unknown> {
  return daemonCliGet<unknown>(zenLocalScope(), 'zen/blob/get', { ...place, ...params })
}
