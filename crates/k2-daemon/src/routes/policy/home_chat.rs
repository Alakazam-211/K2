//! Rows of the route policy table (`routes::route_policy::ROUTES`):
//! the `/cli/home-chat` routes, sorted by path.
//! The floors and what a row means are documented in `route_policy.rs`.

use super::Floor::VoiceChat;
use super::{get, post, Route};

pub(super) const ROWS: &[Route] = &[
    // People and channels on Home (prd-home-people-channels-v1 §13, §14.4;
    // HP-11): every row and the events socket take the VoiceChat floor (a
    // reviewed edit of VOICE_CHAT_ALLOWLIST), so every login reaches them.
    // The handler then takes a person's own login only: the owner token,
    // passports and App sessions get 403. Mgmt changes are checked against
    // K2's ledger (creator, member, Owner or Admin) in the handler.
    post("/cli/home-chat/ack", VoiceChat),
    // 0.46.17: the calls ringing for the caller (a PushKit-woken phone).
    get("/cli/home-chat/calls", VoiceChat),
    get("/cli/home-chat/channels", VoiceChat),
    post("/cli/home-chat/channels/add", VoiceChat),
    post("/cli/home-chat/channels/allow-agents", VoiceChat),
    post("/cli/home-chat/channels/create", VoiceChat),
    post("/cli/home-chat/channels/delete", VoiceChat),
    post("/cli/home-chat/channels/leave", VoiceChat),
    get("/cli/home-chat/channels/members", VoiceChat),
    post("/cli/home-chat/channels/remove", VoiceChat),
    post("/cli/home-chat/channels/rename", VoiceChat),
    post("/cli/home-chat/dm", VoiceChat),
    get("/cli/home-chat/events", VoiceChat),
    // contract-home-chat-attachments-v1: a file's bytes, as the person.
    get("/cli/home-chat/file", VoiceChat),
    get("/cli/home-chat/messages", VoiceChat),
    // prd-home-chat-pushes-v1 §7: a person's own level per conversation and
    // their push choices (every human role; HC-14: one path per method).
    get("/cli/home-chat/notify", VoiceChat),
    post("/cli/home-chat/notify/set", VoiceChat),
    get("/cli/home-chat/people", VoiceChat),
    get("/cli/home-chat/push-prefs", VoiceChat),
    post("/cli/home-chat/push-prefs/set", VoiceChat),
    post("/cli/home-chat/react", VoiceChat),
    get("/cli/home-chat/reactors", VoiceChat),
    // 0.46.18: a DM's recordings, as the member (Sew 0.11.3's client list).
    get("/cli/home-chat/recordings", VoiceChat),
    // prd-home-call-recording-v1 §5: a recording card's transcript lines.
    get("/cli/home-chat/replies", VoiceChat),
    post("/cli/home-chat/send", VoiceChat),
    get("/cli/home-chat/status", VoiceChat),
    get("/cli/home-chat/summary", VoiceChat),
    post("/cli/home-chat/typing", VoiceChat),
    // contract-home-chat-attachments-v1: the person's resumable uploads.
    post("/cli/home-chat/uploads/abort", VoiceChat),
    post("/cli/home-chat/uploads/create", VoiceChat),
    post("/cli/home-chat/uploads/data", VoiceChat),
    get("/cli/home-chat/uploads/status", VoiceChat),
    post("/cli/home-chat/view", VoiceChat),
];
