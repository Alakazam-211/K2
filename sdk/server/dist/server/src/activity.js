// `k2.asGuest(pass).thread.watchActivity(target, cb, opts)` (0.46.5
// activity SDK, review §2.2): the Thread turn of one agent or of many
// rooms, live and right, over `thread.stream` (ear=1) and the snapshot
// reads. It is @k2/activity's `watchActivity` (../../activity/src/core.ts)
// bound to this SDK's socket and calls:
//
// - the snapshot is read when the socket says it is open
//   (`inbox_subscribed`, every (re)open), never on the close-time resync;
//   a frame that arrives while the read is in flight wins over it;
// - `{rooms}`: ONE batched read (`thread.activityRooms`, one rate-limit
//   token for the whole set) instead of one GET per room (10/person burst);
//   `{addr}`: `thread.activity`;
// - the daemon's `activity_reset` (a lagging socket) replaces the state
//   with the turns it carries;
// - a turn shown with no frame for 60 s is read again; a 429 waits
//   `retryAfterMs`; at most one read a second;
// - unsubscribe closes the socket and calls nothing after it returns.
//
// cb gets a Map: room id (or the addr) → {turn, live, source, fresh}.
// `turn` null means nothing is running. Never infer one from messages.
import { watchActivity } from "../../activity/src/core.js";
import { K2Error } from "./errors.js";
function isObj(v) {
    return v !== null && typeof v === 'object' && !Array.isArray(v);
}
/** The transport `watchActivity` drives, for this SDK. */
export function threadActivityTransport(a, target, onError) {
    const byAddr = 'addr' in target;
    const rooms = byAddr ? 'all' : target.rooms === 'all' ? 'all' : target.rooms.join(',');
    if (!byAddr && rooms === '')
        throw new K2Error({ code: 'usage', hint: 'thread.watchActivity: rooms is "all" or a list of room ids', verb: 'thread.watchActivity' });
    // {addr}: the agent's pinned Chat (from the read) and its room (from a frame).
    let conversation = null;
    let room = null;
    return {
        async read() {
            if (byAddr) {
                const v = (await a.call('thread.activity', { addr: target.addr }));
                if (typeof v.conversation === 'string')
                    conversation = v.conversation;
                return { [target.addr]: v.turn ?? null };
            }
            const v = (await a.call('thread.activityRooms', { rooms }));
            return isObj(v.turns) ? v.turns : {};
        },
        connect(on) {
            return a.subscribe('thread.stream', { rooms }, (raw) => {
                if (!isObj(raw))
                    return;
                const f = raw;
                if (f.kind === 'inbox_subscribed') {
                    if (byAddr && isObj(f.resolved)) {
                        for (const [r, c] of Object.entries(f.resolved))
                            if (c === conversation)
                                room = r;
                    }
                    on.open();
                }
                else if (f.kind === 'thread_activity') {
                    if (typeof f.room !== 'string')
                        return;
                    if (!byAddr)
                        return on.frame(f.room, f.activity);
                    if (conversation === null || f.conversation !== conversation)
                        return;
                    room = f.room;
                    on.frame(target.addr, f.activity);
                }
                else if (f.kind === 'activity_reset') {
                    const turns = isObj(f.turns) ? f.turns : null;
                    if (!turns)
                        return on.reset();
                    if (!byAddr)
                        return on.reset(turns);
                    if (room !== null && room in turns)
                        on.reset({ [target.addr]: turns[room] ?? null });
                    else
                        on.reset();
                }
            }, (e) => {
                if (e.code === 'resync')
                    on.drop();
                else
                    onError(e);
            });
        },
    };
}
/** `thread.watchActivity` on one actor. Returns the unsubscribe. */
export function watchThreadActivity(a, target, cb, opts = {}) {
    const report = opts.onError ??
        ((e) => {
            console.warn(`[k2] ${e.message}`);
        });
    return watchActivity(threadActivityTransport(a, target, report), cb, {
        endLingerMs: opts.endLingerMs,
        selfCheckMs: opts.selfCheckMs,
        onVisible: () => () => undefined,
        onError: (err) => report(err instanceof K2Error ? err : new K2Error({ code: 'failed', hint: String(err), verb: 'thread.watchActivity' })),
    });
}
