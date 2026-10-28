import { type ActivityState, type ActivityTransport, type WatchOptions } from '../../activity/src/core.ts';
import { K2Error } from './errors.ts';
export type { ActivityState, Turn } from '../../activity/src/core.ts';
export type WatchActivityTarget = {
    rooms: 'all' | string[];
} | {
    addr: string;
};
export interface WatchActivityOptions extends Pick<WatchOptions, 'endLingerMs' | 'selfCheckMs'> {
    /** A read or the socket failed for good (not a 429, not a resync). */
    onError?: (error: K2Error) => void;
}
/** The two entry points of one actor (`K2App.actor`). */
export interface ActivityActor {
    call: (verb: string, args?: unknown) => Promise<unknown>;
    subscribe: (verb: string, args: unknown, onFrame: (f: unknown) => void, onError?: (e: K2Error) => void) => () => void;
}
/** The transport `watchActivity` drives, for this SDK. */
export declare function threadActivityTransport(a: ActivityActor, target: WatchActivityTarget, onError: (e: K2Error) => void): ActivityTransport;
/** `thread.watchActivity` on one actor. Returns the unsubscribe. */
export declare function watchThreadActivity(a: ActivityActor, target: WatchActivityTarget, cb: (states: Map<string, ActivityState>) => void, opts?: WatchActivityOptions): () => void;
