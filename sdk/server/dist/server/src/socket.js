// A subscribe row over its socket (runtime S23): one WebSocket, frames
// projected onto the row's schema, reconnects after a drop.
//
// K2's App sockets send no hello and no replay. After any gap (a drop, or
// the 4008 "lagged" close), onError gets a `resync` K2Error naming the verb
// to refetch with, and the socket reconnects (backoff 0.5 s → 10 s); catch
// up in `onOpen` (every open, 0.46.5), which comes after the gap. A
// socket refused before it ever opened (no room, no cap, a dead pass)
// ends with one `failed` error and no retry.
//
// When access goes while a socket is open (0.46.8):
// - 4013: the pass is fine, this socket's room or cap is gone. It ends with
//   one `access_lost` error and no retry. Read me.get again.
// - A re-dial that doesn't open: neither a browser nor Node says why, and a
//   dead pass, a lost room, a lost close code and a restarting K2 all look
//   the same here. So it asks: me.get with the same pass, over HTTP.
//     - The pass is refused: one `failed` with its status (401), no retry.
//     - K2 answers, and the socket still won't open: [MAX_REFUSED] dials in a
//       row, then one `failed`. It never loops: each refused upgrade is a row
//       in the owner's audit.
//     - K2 doesn't answer (a restart, a network drop): keep retrying with no
//       error, however long. Nobody's access changed.
// - 4003: the pass is dead, or an App's own pass rotated. Same question first,
//   so a dead pass costs no refused upgrade and a rotated one reopens.
import { K2Error } from "./errors.js";
import { project } from "./wire.js";
const BACKOFF_MS = [500, 1000, 2000, 5000, 10000];
/** Dials in a row that K2 refused while it answered for the pass. A socket quota or a
 *  moment in a daemon handoff can refuse some; with the backoff above, this many
 *  takes about 30 s and is an answer. */
export const MAX_REFUSED = 6;
/** The pass is dead: K2 said so about the pass itself. */
const CLOSE_PASS_GONE = 4003;
/** The pass is fine, this socket's room or cap is gone. */
const CLOSE_ACCESS_LOST = 4013;
/** What to refetch with after a gap: the row's verb, plus `thread.activity`
 *  for the Thread turn on a socket opened with the ear (`ear=1`). */
export function refetchHint(row) {
    const verbs = [row.socket?.refetch, row.socket?.query?.ear === '1' ? 'thread.activity' : undefined].filter((v) => typeof v === 'string' && v.length > 0);
    return verbs.length ? verbs.join(' and ') : null;
}
async function askAboutPass(check) {
    if (!check)
        return { kind: 'accepted' };
    try {
        await check();
        return { kind: 'accepted' };
    }
    catch (e) {
        if (e instanceof K2Error && (e.status === 401 || e.status === 403))
            return { kind: 'refused', error: e };
        return { kind: 'unknown' };
    }
}
export function openSocket(o) {
    const refetch = refetchHint(o.row);
    let stopped = false;
    let everOpened = false;
    let attempt = 0;
    let refused = 0;
    let ws = null;
    let timer = null;
    const fail = (e) => {
        try {
            o.onError(e);
        }
        catch (err) {
            // A throwing onError must not kill the socket loop; surface it.
            queueMicrotask(() => {
                throw err;
            });
        }
    };
    const end = (init) => {
        stopped = true;
        fail(new K2Error({ ...init, verb: o.verb }));
    };
    const passDead = (e) => {
        end({
            code: 'failed',
            status: e.status,
            hint: `${o.verb}: the socket ended because K2 refuses this pass (${e.code}); read K2_PASS_FILE again, or sign the person in again`,
        });
    };
    const redial = () => {
        const wait = BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)] ?? 10000;
        attempt += 1;
        timer = setTimeout(() => {
            timer = null;
            if (!stopped)
                connect();
        }, wait);
    };
    const saidGap = (why) => {
        fail(new K2Error({
            code: 'resync',
            hint: `${o.verb}: ${why}; reconnecting${refetch ? `, refetch with ${refetch}` : ''}`,
            verb: o.verb,
        }));
    };
    /** A dial after the first open that never opened. */
    const redialFailed = () => {
        void askAboutPass(o.checkPass).then((answer) => {
            if (stopped)
                return;
            if (answer.kind === 'refused') {
                passDead(answer.error);
                return;
            }
            if (answer.kind === 'accepted') {
                refused += 1;
                if (refused >= MAX_REFUSED) {
                    end({
                        code: 'failed',
                        hint: `${o.verb}: K2 keeps refusing this socket though the pass still works: its room, capability or conversation is gone, or the App is out of sockets; read me.get again`,
                    });
                    return;
                }
            }
            redial();
        });
    };
    const connect = () => {
        let url;
        try {
            url = o.url();
        }
        catch (e) {
            stopped = true;
            fail(e instanceof K2Error ? e : new K2Error({ code: 'failed', hint: String(e), verb: o.verb }));
            return;
        }
        const sock = new o.WebSocket(url);
        ws = sock;
        let opened = false;
        let dialEnded = false;
        sock.addEventListener('open', () => {
            opened = true;
            everOpened = true;
            attempt = 0;
            refused = 0;
            if (stopped || !o.onOpen)
                return;
            try {
                o.onOpen();
            }
            catch (err) {
                queueMicrotask(() => {
                    throw err;
                });
            }
        });
        sock.addEventListener('message', (ev) => {
            if (typeof ev.data !== 'string')
                return;
            let v;
            try {
                v = JSON.parse(ev.data);
            }
            catch {
                return; // not a frame
            }
            o.onFrame(project(o.row.response, v, o.row.responseNames));
        });
        const notOpened = () => {
            if (stopped || dialEnded)
                return;
            dialEnded = true;
            if (everOpened) {
                redialFailed();
                return;
            }
            end({
                code: 'failed',
                hint: `${o.verb}: the socket was refused before it opened (check the room, its cap and the pass)`,
            });
        };
        // A refused handshake (a non-101 answer): some runtimes (Node 22's
        // WebSocket) report it with `error` and never fire `close`, so a
        // refusal must not wait for `close`.
        sock.addEventListener('error', () => {
            if (!opened)
                notOpened();
        });
        sock.addEventListener('close', (ev) => {
            if (stopped)
                return;
            if (!opened) {
                notOpened();
                return;
            }
            if (ev.code === CLOSE_ACCESS_LOST) {
                end({ code: 'access_lost', hint: `${o.verb}: access to this room or capability ended (4013); read me.get again, and don't reopen this socket` });
                return;
            }
            if (ev.code === CLOSE_PASS_GONE) {
                void askAboutPass(o.checkPass).then((answer) => {
                    if (stopped)
                        return;
                    if (answer.kind === 'refused') {
                        passDead(answer.error);
                        return;
                    }
                    saidGap('it closed (4003)');
                    redial();
                });
                return;
            }
            saidGap(ev.code === 4008 ? 'K2 dropped it for lagging (4008)' : `it closed (${ev.code})`);
            redial();
        });
    };
    connect();
    return () => {
        stopped = true;
        if (timer)
            clearTimeout(timer);
        if (ws && ws.readyState <= 1)
            ws.close(1000, 'unsubscribed');
    };
}
