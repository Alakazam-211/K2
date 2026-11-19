//! A native doc's cut in its session (feature `native`): what a commit tells the manager's cut
//! queue, and the cuts the manager runs through the session.
//!
//! - **After each commit** of a native doc's records (a change, an outside save, a purge), the
//!   session stamps the commit's time in the manager's inbox and asks the doc whether a cut is
//!   due (the native engine says when a cut is due, in O(1); it cuts nothing). A due doc goes in
//!   the inbox with its reason and tail. No cut runs on an edit's path.
//! - **The queue's job** ([`Session::cut_now`]) runs under the session lock, between edits: the
//!   doc's cut when there is a change since its last one (the native engine's cut on its idle
//!   tick).
//! - **A clean unload** ([`Session::unload_cut`]) cuts a doc with a change since its last cut
//!   before it leaves memory (the native engine's cut before it unloads).

use std::sync::Arc;

use super::Session;
use crate::session::cut_queue::NativeCutInbox;

impl Session {
    /// The manager's cut inbox (module docs), set once at open.
    pub(crate) fn set_cut_inbox(&self, inbox: Arc<NativeCutInbox>) {
        let _ = self.cut_inbox.set(inbox);
    }
}
