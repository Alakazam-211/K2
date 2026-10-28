import { K2Error } from './errors.ts';
import type { AppVerbRow } from './generated/verbs.ts';
export type WebSocketCtor = new (url: string) => WebSocket;
export interface SocketOpts {
    verb: string;
    row: AppVerbRow;
    /** The URL to dial now (the pass is read again on every reconnect). */
    url: () => string;
    onFrame: (frame: unknown) => void;
    onError: (error: K2Error) => void;
    /** 0.46.5 (review D5): the socket is open, first time and after every
     *  reconnect. Catch up HERE (read the snapshot after the socket is
     *  open), not on the close-time `resync`, which comes before the gap. */
    onOpen?: () => void;
    WebSocket: WebSocketCtor;
}
/** What to refetch with after a gap: the row's verb, plus `thread.activity`
 *  for the Thread turn on a socket opened with the ear (`ear=1`). */
export declare function refetchHint(row: AppVerbRow): string | null;
export declare function openSocket(o: SocketOpts): () => void;
