// 0.40.22 large-file transfers — byte-level progress store.
//
// Toasts carry one-shot messages; long transfers (multi-GB uploads,
// server-side compress jobs, downloads to the local machine) need a LIVE
// percent the whole time the bytes move. This store is the single place
// every transfer loop reports into; `TransferProgress` renders whatever is
// in-flight. Cancellation is cooperative: the Cancel button only raises
// `cancelRequested`, and the driving loop polls it between chunks — the
// natural safe stopping points of every chunked transfer.

import { create } from 'zustand'

export type TransferKind = 'upload' | 'download' | 'compress' | 'extract'

export interface Transfer {
  id: string
  kind: TransferKind
  /** Display name — the file/folder the bytes belong to. */
  label: string
  /** 0..1 completed fraction; null = indeterminate (not yet measurable). */
  fraction: number | null
  /** Raised by the Cancel affordance; polled by the transfer loop. */
  cancelRequested: boolean
}

interface TransferProgressState {
  transfers: Transfer[]
  /** Register a transfer; returns its id for update/end calls. */
  begin: (kind: TransferKind, label: string) => string
  update: (id: string, fraction: number | null) => void
  /** Remove a finished/failed/cancelled transfer from the overlay. */
  end: (id: string) => void
  requestCancel: (id: string) => void
  isCancelRequested: (id: string) => boolean
}

export const useTransferProgressStore = create<TransferProgressState>((set, get) => ({
  transfers: [],

  begin: (kind, label) => {
    const id = crypto.randomUUID()
    set((s) => ({
      transfers: [...s.transfers, { id, kind, label, fraction: null, cancelRequested: false }],
    }))
    return id
  },

  update: (id, fraction) => {
    set((s) => ({
      transfers: s.transfers.map((t) => (t.id === id ? { ...t, fraction } : t)),
    }))
  },

  end: (id) => {
    set((s) => ({ transfers: s.transfers.filter((t) => t.id !== id) }))
  },

  requestCancel: (id) => {
    set((s) => ({
      transfers: s.transfers.map((t) => (t.id === id ? { ...t, cancelRequested: true } : t)),
    }))
  },

  isCancelRequested: (id) => {
    return get().transfers.some((t) => t.id === id && t.cancelRequested)
  },
}))
