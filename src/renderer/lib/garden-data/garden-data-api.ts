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

// ── the person's views (Settings → Gardens → Widget data) ─────────

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

/** "It's mine": stamp the live version of a widget as the person's. */
export function claimWidget(widget: string): Promise<{ widget: string; hash: string }> {
  return daemonCliPost<{ widget: string; hash: string }>(zenLocalScope(), 'zen/widget/claim', { widget })
}

// ── the widget door (B2's bridge uses this) ──────────────────────────

export function gardenCall<T = unknown>(body: GardenCallBody): Promise<GardenCallResponse<T>> {
  return daemonCliPost<GardenCallResponse<T>>(zenLocalScope(), 'zen/call', body)
}
