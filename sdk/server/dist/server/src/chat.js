// App-governed Spaces helpers (prd-app-governed-spaces-k2-v1 §5.4).
// Hand-written, beside the generated verbs `chat.channels` (Sew's bulk
// channel ops, at most 200 a call) and `chat.setChannels` (each person's
// App-managed channel list in full, at most 500 people a call):
//
//   const app = k2.asApp()
//   await applyChannelOps(app, [
//     channelOp.ensure('job-1042', 'Job 1042', { private: true }),
//     channelOp.ensure('site-call', 'Site call', { type: 'voice', userLimit: 12 }),
//     channelOp.archive('job-0917'),
//   ])
//   await setAllChannels(app, [{ username: 'dana', channels: ['job-1042'] }])
//
// - The builders check each op the way K2 does before Sew sees it, and
//   throw a K2Error with code `usage` on a bad one (nothing is sent).
// - The two senders split a long list into calls of the allowed size, in
//   order, and join the answers. They stop at the first refused call and
//   throw its K2Error (the calls before it went through). Both verbs share
//   one limit of 60 calls a minute per App: a 429 throws `rate_limited`
//   with `retryAfterSecs`.
// - A person's list from setChannels is the App's own; K2 sends the
//   person's role channels (the manifest's `chat` block) plus it, so the
//   two never overwrite each other. An empty list clears the App's list.
import { K2Error } from "./errors.js";
/** Most ops in one `chat.channels` call. */
export const CHANNEL_OPS_PER_CALL = 200;
/** Most people in one `chat.setChannels` call. */
export const MEMBER_CHANNELS_PER_CALL = 500;
/** Most channel slugs in one person's list. */
export const CHANNELS_PER_PERSON = 5000;
const SLUG = /^[a-z0-9][a-z0-9-]{0,79}$/;
function usage(verb, hint) {
    return new K2Error({ code: 'usage', hint: `${verb}: ${hint}`, verb });
}
function slugOk(verb, slug) {
    if (typeof slug !== 'string' || !SLUG.test(slug)) {
        throw usage(verb, `'${slug}' isn't a channel slug (1 to 80 of a-z, 0-9 and -, starting with a letter or digit)`);
    }
    return slug;
}
function nameOk(verb, name) {
    const n = typeof name === 'string' ? name.trim() : '';
    if (n.length === 0 || [...n].length > 100)
        throw usage(verb, 'a channel name is 1 to 100 characters');
    return n;
}
function ref(verb, r) {
    if (typeof r === 'string')
        return { slug: slugOk(verb, r) };
    if (r && typeof r.channelId === 'string' && /^\d{1,20}$/.test(r.channelId))
        return { channel_id: r.channelId };
    throw usage(verb, 'name the channel by its slug, or { channelId: "<Sew id>" }');
}
/** Builders for `chat.channels` ops. */
export const channelOp = {
    /** Make the channel, or find it: the same slug again answers `created: false` and never renames. */
    ensure(slug, name, opts = {}) {
        const verb = 'channelOp.ensure';
        const type = opts.type ?? 'text';
        if (type !== 'text' && type !== 'voice')
            throw usage(verb, "type is 'text' or 'voice'");
        const op = { op: 'ensure', slug: slugOk(verb, slug), name: nameOk(verb, name), type };
        if (opts.private !== undefined)
            op.private = opts.private === true;
        if (opts.userLimit !== undefined) {
            if (type !== 'voice')
                throw usage(verb, 'userLimit is for voice channels');
            if (!Number.isInteger(opts.userLimit) || opts.userLimit < 0 || opts.userLimit > 99)
                throw usage(verb, 'userLimit is a whole number from 0 to 99');
            op.user_limit = opts.userLimit;
        }
        return op;
    },
    /** A new name; the slug stays. */
    rename(channel, name) {
        return { op: 'rename', ...ref('channelOp.rename', channel), name: nameOk('channelOp.rename', name) };
    },
    /** Archive it: history is kept, nobody posts. Never a delete. */
    archive(channel) {
        return { op: 'archive', ...ref('channelOp.archive', channel) };
    },
    /** Bring an archived channel back. */
    unarchive(channel) {
        return { op: 'unarchive', ...ref('channelOp.unarchive', channel) };
    },
};
/**
 * Send any number of channel ops, 200 a call, in order. The answer joins
 * the calls' `results` (one per op, in order). A refused call throws its
 * K2Error; the ops before it went through.
 */
export async function applyChannelOps(app, ops, opts = {}) {
    if (!Array.isArray(ops) || ops.length === 0)
        throw usage('applyChannelOps', 'give at least one op (channelOp.ensure, rename, archive or unarchive)');
    let last;
    const results = [];
    for (let i = 0; i < ops.length; i += CHANNEL_OPS_PER_CALL) {
        const args = { ops: ops.slice(i, i + CHANNEL_OPS_PER_CALL) };
        if (opts.space !== undefined)
            args.space = opts.space;
        last = await app.chat.channels(args);
        results.push(...last.results);
    }
    return { ...last, results };
}
/**
 * Set each person's App-managed channel list (in full), 500 people a call,
 * in order. Each username once; each list at most 5,000 slugs. The answer
 * joins the calls' `items` and adds up `changed`. A person who isn't this
 * App's user comes back `{ok: false, error: 'not_found'}`, an agent's
 * login `agent_subject`; the rest still apply.
 */
export async function setAllChannels(app, items, opts = {}) {
    const verb = 'setAllChannels';
    if (!Array.isArray(items) || items.length === 0)
        throw usage(verb, 'give at least one {username, channels}');
    const seen = new Set();
    for (const it of items) {
        const u = typeof it?.username === 'string' ? it.username.trim().toLowerCase() : '';
        if (u === '')
            throw usage(verb, 'each item names a username');
        if (seen.has(u))
            throw usage(verb, `'${it.username}' is listed twice: one list per person`);
        seen.add(u);
        if (!Array.isArray(it.channels) || it.channels.length > CHANNELS_PER_PERSON)
            throw usage(verb, `${it.username}: channels is a list of at most ${CHANNELS_PER_PERSON} slugs`);
        for (const c of it.channels)
            slugOk(verb, c);
    }
    let last;
    let changed = 0;
    const out = [];
    for (let i = 0; i < items.length; i += MEMBER_CHANNELS_PER_CALL) {
        const args = { items: items.slice(i, i + MEMBER_CHANNELS_PER_CALL) };
        if (opts.space !== undefined)
            args.space = opts.space;
        last = await app.chat.setChannels(args);
        changed += last.changed;
        out.push(...last.items);
    }
    return { ...last, changed, items: out };
}
