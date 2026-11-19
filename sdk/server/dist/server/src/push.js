// App chat pushes (prd-home-chat-pushes-v1 HC-25, 0.46.16). Hand-written,
// beside the generated verbs: the `data` K2 puts on a DM or channel push to
// the App's own phone app (`k2Kind: chat`), read back into one typed value,
// and the `push.viewing` conversation for a chat.
//
//   // the phone app (or its server) on a notification tap:
//   const chat = chatPushFrom(notification.data)
//   if (chat) openChat(chat.conversation, { scrollTo: chat.messageId })
//
//   // while that chat is on screen, every 30 s, from the App's server:
//   await k2.asGuest(pass).push.viewing({ conversation: chatConversation(id), visible: true })
//
// The owner, an Admin or the App's agent turns these pushes on with
// `k2 app push set <app> --chat on` (off by default; the server also needs
// K2_APP_CHAT_PUSH=1, and `/boot-status` lists `app-chat-push-v1` then).
/** `push.viewing`'s conversation for an App chat. */
export const CHAT_CONVERSATION_PREFIX = 'chat:';
/** The Android notification channel id chat pushes use. */
export const CHAT_ANDROID_CHANNEL = 'chat';
const REASONS = ['dm', 'mention', 'reply', 'all', 'keyword'];
const SEW_ID = /^[A-Za-z0-9_-]{1,64}$/;
/**
 * A push's data map as a {@link ChatPush}, or `null` when it isn't a chat
 * push (another `k2Kind`, or a field missing or malformed).
 */
export function chatPushFrom(data) {
    if (!data || data.k2Kind !== 'chat')
        return null;
    const s = (k) => (typeof data[k] === 'string' ? data[k] : null);
    const app = s('k2App');
    const reason = s('k2Reason');
    const conversation = s('k2Conv');
    const kind = s('k2ConvKind');
    const messageId = s('k2Msg');
    const count = Number(s('k2Count'));
    const mention = s('k2Mention');
    const who = s('k2Who');
    if (!app || !who || !reason || !REASONS.includes(reason))
        return null;
    if (!conversation || !SEW_ID.test(conversation) || !messageId || !SEW_ID.test(messageId))
        return null;
    if (kind !== 'dm' && kind !== 'channel')
        return null;
    if (!Number.isInteger(count) || count < 1 || (mention !== '0' && mention !== '1'))
        return null;
    return { app, reason: reason, conversation, kind, messageId, count, mention: mention === '1', who };
}
/** `chat:<id>`, the conversation `push.viewing` takes for an App chat. */
export function chatConversation(channelId) {
    if (!SEW_ID.test(channelId))
        throw new Error('chatConversation: a Sew channel id is 1 to 64 of A-Z, a-z, 0-9, _ and - (k2Conv on a chat push)');
    return `${CHAT_CONVERSATION_PREFIX}${channelId}`;
}
