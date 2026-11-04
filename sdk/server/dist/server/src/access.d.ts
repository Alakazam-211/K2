import { K2Error } from './errors.ts';
import type { AppMe } from './generated/verbs.ts';
export type AccessChange = {
    me: AppMe;
    ended: false;
} | {
    me: null;
    ended: true;
};
export interface OnAccessChangedOptions {
    /** A read failed: it keeps listening. Or the socket stopped and won't retry, for a
     *  reason other than a dead pass (`failed` with status 0: a server with no
     *  `me.events`, or out of sockets): call `onAccessChanged` again later. */
    onError?: (error: K2Error) => void;
}
/** The two entry points of one actor. */
export interface AccessActor {
    call: (verb: string, args?: unknown) => Promise<unknown>;
    subscribe: (verb: string, args: unknown, onFrame: (f: unknown) => void, onError?: (e: K2Error) => void, opts?: {
        onOpen?: () => void;
    }) => () => void;
}
export declare function watchAccess(a: AccessActor, cb: (change: AccessChange) => void, opts?: OnAccessChangedOptions): () => void;
export interface OnGuestAccessChangedOptions {
    /** The socket stopped and won't retry. Frames aren't replayed. */
    onError?: (error: K2Error) => void;
    /** The socket is open, first time and after every reconnect: read `me.get` again
     *  for each guest you show. Nothing is replayed. */
    onOpen?: () => void;
}
/** `scope=guests`: one socket for all of this App's guests. */
export declare function watchGuestAccess(a: AccessActor, cb: (guest: {
    principalId: string;
}) => void, opts?: OnGuestAccessChangedOptions): () => void;
