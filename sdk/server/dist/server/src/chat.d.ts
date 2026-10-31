import type { AppChatChannelOp, AppChatChannelOps, AppChatMemberChannels, AppChatMemberChannelsResult } from './generated/verbs.ts';
/** Most ops in one `chat.channels` call. */
export declare const CHANNEL_OPS_PER_CALL = 200;
/** Most people in one `chat.setChannels` call. */
export declare const MEMBER_CHANNELS_PER_CALL = 500;
/** Most channel slugs in one person's list. */
export declare const CHANNELS_PER_PERSON = 5000;
/** What the senders need: `k2.asApp()` (these verbs take the App's own pass, never a guest's). */
export interface ChatChannelsActor {
    chat: {
        channels(args: {
            ops: AppChatChannelOp[];
            space?: string;
        }): Promise<AppChatChannelOps>;
        setChannels(args: {
            items: AppChatMemberChannels[];
            space?: string;
        }): Promise<AppChatMemberChannelsResult>;
    };
}
/** Which Space (one of the App's own; leave it out when the App has only one). */
export interface ChatSpaceOptions {
    space?: string;
}
/** A channel named by its slug, or by Sew's id (`{ channelId }`). */
export type ChannelRef = string | {
    channelId: string;
};
export interface EnsureOptions {
    /** `text` (default) or `voice`. A channel never changes type. */
    type?: 'text' | 'voice';
    /** Members only (default false: every member of the Space sees it). */
    private?: boolean;
    /** Voice only: at most this many people (0 to 99; 0 = no limit). */
    userLimit?: number;
}
/** Builders for `chat.channels` ops. */
export declare const channelOp: {
    /** Make the channel, or find it: the same slug again answers `created: false` and never renames. */
    ensure(slug: string, name: string, opts?: EnsureOptions): AppChatChannelOp;
    /** A new name; the slug stays. */
    rename(channel: ChannelRef, name: string): AppChatChannelOp;
    /** Archive it: history is kept, nobody posts. Never a delete. */
    archive(channel: ChannelRef): AppChatChannelOp;
    /** Bring an archived channel back. */
    unarchive(channel: ChannelRef): AppChatChannelOp;
};
/**
 * Send any number of channel ops, 200 a call, in order. The answer joins
 * the calls' `results` (one per op, in order). A refused call throws its
 * K2Error; the ops before it went through.
 */
export declare function applyChannelOps(app: ChatChannelsActor, ops: readonly AppChatChannelOp[], opts?: ChatSpaceOptions): Promise<AppChatChannelOps>;
/**
 * Set each person's App-managed channel list (in full), 500 people a call,
 * in order. Each username once; each list at most 5,000 slugs. The answer
 * joins the calls' `items` and adds up `changed`. A person who isn't this
 * App's user comes back `{ok: false, error: 'not_found'}`, an agent's
 * login `agent_subject`; the rest still apply.
 */
export declare function setAllChannels(app: ChatChannelsActor, items: readonly AppChatMemberChannels[], opts?: ChatSpaceOptions): Promise<AppChatMemberChannelsResult>;
