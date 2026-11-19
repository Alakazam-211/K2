/** Why K2 pushed: a DM, an @mention, a reply, or (Sew 0.11.0) every message or a keyword. */
export type ChatPushReason = 'dm' | 'mention' | 'reply' | 'all' | 'keyword';
/** A chat push's `data`, typed. Ids only: no name or text ever rides here. */
export interface ChatPush {
    /** The App's service id (`k2App`). */
    app: string;
    reason: ChatPushReason;
    /** The Sew channel id (`k2Conv`); a DM is a channel too. */
    conversation: string;
    /** `dm` or `channel` (`k2ConvKind`). */
    kind: 'dm' | 'channel';
    /** The newest message the push covers (`k2Msg`): scroll to it. */
    messageId: string;
    /** How many messages it covers (`k2Count`, at least 1). */
    count: number;
    /** Any of them mentions the person (`k2Mention`). */
    mention: boolean;
    /** The opaque per-(App, person) key (`k2Who`): match it to `push.register`'s `who`. */
    who: string;
}
/** `push.viewing`'s conversation for an App chat. */
export declare const CHAT_CONVERSATION_PREFIX = "chat:";
/** The Android notification channel id chat pushes use. */
export declare const CHAT_ANDROID_CHANNEL = "chat";
/**
 * A push's data map as a {@link ChatPush}, or `null` when it isn't a chat
 * push (another `k2Kind`, or a field missing or malformed).
 */
export declare function chatPushFrom(data: Record<string, unknown> | null | undefined): ChatPush | null;
/** `chat:<id>`, the conversation `push.viewing` takes for an App chat. */
export declare function chatConversation(channelId: string): string;
