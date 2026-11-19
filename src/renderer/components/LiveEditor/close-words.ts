// The Live editor's words for why a connection ended (WIRE.md, Closing), and the note after a
// rollback. Kept apart from mount.ts, which holds the editor itself.
import type { CloseReason } from "@k2/mpc-client";

export const CLOSED_WORDS: Record<CloseReason, string> = {
  left: "You left this document.",
  idle: "Disconnected after a while without activity.",
  kicked: "You were removed from this document.",
  rights_lost: "You no longer have access to this document.",
  deleted: "This document was deleted.",
  moved: "This document was moved.",
  released: "This document was handed over.",
  cut_off: "The connection fell too far behind.",
  declined: "This document can't be edited live.",
  not_found: "This document wasn't found.",
  auth: "Sign in again to keep editing.",
  network: "The connection was lost.",
  closed: "Live editing of this document was closed.",
  rolled_back: "Saving to disk failed, so this document went back to what was last saved.",
};

/** Kit v0.23.0, grouped saves: the host rolled the doc back to what was on disk after a disk
 * failure (a `rolled_back` close); the editor shows this and joins again. */
export const ROLLED_BACK_NOTE =
  "Saving to disk failed, so this document went back to what was last saved: your last fraction of a second of edits was rolled back. Check your last change and make it again if it's missing.";
