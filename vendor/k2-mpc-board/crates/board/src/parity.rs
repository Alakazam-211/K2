//! The parity list: everything a person can do on a board, and how an agent does it. An action and
//! its row are one line here, and `tests/parity.rs` matches on every action, so a new action can't
//! land without a way and a test. `docs/PARITY.md` is generated from this. The actions come from
//! Board's own list, Copyright (c) 2026 Baden Ballard, MIT (see `THIRD-PARTY-NOTICES.md`).

/// How an agent does it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// Through these verbs.
    Verbs(&'static [&'static str]),
    /// It changes nothing in the file, so it needs no verb. Says why.
    ViewOnly(&'static str),
    /// A file doesn't hold it, by design. Says why.
    NotInFile(&'static str),
    /// It's the host's, not the file's. Says why.
    Host(&'static str),
    /// It comes with live. Says why.
    LiveLater(&'static str),
}

/// One row of the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    /// The action.
    pub action: PersonAction,
    /// Its stable name.
    pub name: &'static str,
    /// In words.
    pub person: &'static str,
    /// How an agent does it.
    pub way: Way,
}

macro_rules! actions {
    ($($variant:ident, $name:literal, $says:literal, $way:expr;)*) => {
        /// One thing a person can do.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum PersonAction { $(#[doc = $says] $variant,)* }

        /// The list, one row per action.
        pub const ROWS: &[Row] = &[
            $(Row { action: PersonAction::$variant, name: $name, person: $says, way: $way },)*
        ];
    };
}

actions! {
    AddSticky, "add-sticky", "Add a sticky note", Way::Verbs(&["add"]);
    AddText, "add-text", "Add a text block", Way::Verbs(&["add"]);
    AddShape, "add-shape", "Add a shape", Way::Verbs(&["add"]);
    DrawPen, "draw-pen", "Draw with the pen or highlighter", Way::Verbs(&["add"]);
    AddFrame, "add-frame", "Add a frame", Way::Verbs(&["add"]);
    AddCard, "add-card", "Add a card", Way::Verbs(&["add"]);
    AddTable, "add-table", "Add a table", Way::Verbs(&["add"]);
    AddMindNode, "add-mind-node", "Add a mind-map node", Way::Verbs(&["add"]);
    AddImage, "add-image", "Add a picture", Way::Verbs(&["add"]);
    PasteText, "paste-text", "Paste text as stickies or a text block", Way::Verbs(&["add", "apply"]);
    EditFields, "edit-fields", "Edit text, colours, sizes, alignment, tags, links, crops and card fields", Way::Verbs(&["update"]);
    Resize, "resize", "Resize", Way::Verbs(&["update"]);
    Rotate, "rotate", "Rotate", Way::Verbs(&["update"]);
    MoveObjects, "move-objects", "Move or nudge, with groups and frame contents", Way::Verbs(&["move"]);
    DrawConnector, "draw-connector", "Draw a connector", Way::Verbs(&["connect"]);
    ReattachConnector, "reattach-connector", "Drag a connector end to another object", Way::Verbs(&["update"]);
    Delete, "delete", "Delete, with what hangs off the object", Way::Verbs(&["delete"]);
    EraseStroke, "erase-stroke", "Erase a pen stroke", Way::Verbs(&["delete"]);
    Cut, "cut", "Cut", Way::Verbs(&["delete", "list"]);
    Copy, "copy", "Copy and paste", Way::Verbs(&["list", "paste"]);
    Duplicate, "duplicate", "Duplicate", Way::Verbs(&["duplicate"]);
    ConnectedCopy, "connected-copy", "Add a connected copy from a connection dot", Way::Verbs(&["connected-copy"]);
    CopyStyle, "copy-style", "Copy style", Way::Verbs(&["copy-style"]);
    Tidy, "tidy", "Tidy up", Way::Verbs(&["tidy"]);
    Align, "align", "Align", Way::Verbs(&["align"]);
    Distribute, "distribute", "Distribute", Way::Verbs(&["distribute"]);
    Group, "group", "Group", Way::Verbs(&["group"]);
    Ungroup, "ungroup", "Ungroup", Way::Verbs(&["ungroup"]);
    Lock, "lock", "Lock", Way::Verbs(&["lock"]);
    Unlock, "unlock", "Unlock", Way::Verbs(&["lock"]);
    BringToFront, "bring-to-front", "Bring to front", Way::Verbs(&["order"]);
    SendToBack, "send-to-back", "Send to back", Way::Verbs(&["order"]);
    BringForward, "bring-forward", "Bring forward", Way::Verbs(&["order"]);
    SendBackward, "send-backward", "Send backward", Way::Verbs(&["order"]);
    ReorderFrames, "reorder-frames", "Reorder frames for presenting", Way::Verbs(&["frame-order"]);
    Comment, "comment", "Add a comment, pinned or free", Way::Verbs(&["comment"]);
    Reply, "reply", "Reply to a comment", Way::Verbs(&["reply"]);
    Resolve, "resolve", "Resolve or reopen a comment", Way::Verbs(&["resolve"]);
    DeleteComment, "delete-comment", "Delete a comment", Way::Verbs(&["delete"]);
    React, "react", "Add or take off a reaction", Way::Verbs(&["react"]);
    Stamp, "stamp", "Stamp an object, as a reaction drawn big", Way::Verbs(&["react"]);
    Vote, "vote", "Vote", Way::Verbs(&["vote"]);
    TakeBackVote, "take-back-vote", "Take back a vote", Way::Verbs(&["vote"]);
    StartVoting, "start-voting", "Start or stop voting", Way::Verbs(&["meta"]);
    RenameBoard, "rename-board", "Change the board title", Way::Verbs(&["meta"]);
    AddCheck, "add-check", "Add a check item", Way::Verbs(&["check-add"]);
    TickCheck, "tick-check", "Tick or edit a check item", Way::Verbs(&["check-set"]);
    RemoveCheck, "remove-check", "Remove a check item", Way::Verbs(&["check-remove"]);
    EditCell, "edit-cell", "Edit a table cell", Way::Verbs(&["table-cell"]);
    AddRow, "add-row", "Add a table row", Way::Verbs(&["table-add-row"]);
    RemoveRow, "remove-row", "Remove a table row", Way::Verbs(&["table-remove-row"]);
    AddColumn, "add-column", "Add a table column", Way::Verbs(&["table-add-column"]);
    RemoveColumn, "remove-column", "Remove a table column", Way::Verbs(&["table-remove-column"]);
    MindChild, "mind-child", "Add a mind-map child", Way::Verbs(&["mind-child"]);
    MindSibling, "mind-sibling", "Add a mind-map sibling", Way::Verbs(&["mind-child"]);
    UseTemplate, "use-template", "Add a template", Way::Verbs(&["templates", "template"]);
    Search, "search", "Search the board", Way::Verbs(&["search"]);
    ReadBoard, "read-board", "Look at the board and read an object", Way::Verbs(&["list", "read", "fields", "references"]);
    SeeAuthor, "see-author", "See who made an object", Way::Verbs(&["read"]);
    Undo, "undo", "Undo", Way::Verbs(&["inverse"]);
    Redo, "redo", "Redo", Way::Verbs(&["inverse"]);
    RestoreDeleted, "restore-deleted", "Undo a delete", Way::Verbs(&["put-back"]);
    ClearMissingPicture, "clear-missing-picture", "Clear a picture whose bytes are missing", Way::Verbs(&["clear-blob"]);
    PasteMany, "paste-many", "Paste or draw several things as one step", Way::Verbs(&["apply"]);
    View, "view", "Pan, zoom, fit, minimap, present", Way::ViewOnly("It changes nothing in the file.");
    Select, "select", "Select, marquee", Way::ViewOnly("It changes nothing in the file. A verb names its objects. Showing a selection to others is live.");
    SnapGuides, "snap-guides", "Smart guides and grid snap", Way::ViewOnly("They pick numbers for a person dragging. A verb gives the numbers.");
    Appearance, "appearance", "Light and dark", Way::ViewOnly("It changes nothing in the file.");
    Timer, "timer", "Run a timer", Way::NotInFile("A running timer is out of the file by design: its end time is stale once sent. It's the host's.");
    History, "history", "Board history", Way::NotInFile("A file holds no history. Version control or the live session holds it.");
    Export, "export", "Export PNG, PDF", Way::Host("It needs a renderer, so it's the host's. The file itself is the JSON export.");
    OpenLink, "open-link", "Open a link on an object", Way::Host("Opening anything is the host's. `read` gives the address.");
    Access, "access", "Who can open a board, projects, trash", Way::Host("Access and places are the host's: a file carries neither.");
    Save, "save", "Save", Way::Host("The host owns the file, through `BoardFile`, with the guard.");
    Presence, "presence", "See who's here, their cursors, follow them", Way::LiveLater("One writer at a time, with no presence. It comes with live.");
}

/// The bytes of `docs/PARITY.md`.
pub fn parity_markdown() -> String {
    let mut out = String::from(
        "# Agent parity\n\nEverything a person can do on a board, and how an agent does it, at file level with no live session. `tests/parity.rs` runs each verb row the person's way, as the raw ops a canvas sends, and the agent's way, and the boards must match. This file is generated from `crates/board/src/parity.rs`. The list of actions is Board's own: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT; see `THIRD-PARTY-NOTICES.md`.\n\n| Action | A person | An agent |\n|---|---|---|\n",
    );
    for row in ROWS {
        let way = match row.way {
            Way::Verbs(verbs) => verbs
                .iter()
                .map(|v| format!("`{v}`"))
                .collect::<Vec<_>>()
                .join(", "),
            Way::ViewOnly(why) => format!("None needed. {why}"),
            Way::NotInFile(why) => format!("Not in a file. {why}"),
            Way::Host(why) => format!("The host's. {why}"),
            Way::LiveLater(why) => format!("With live. {why}"),
        };
        out.push_str(&format!("| `{}` | {} | {way} |\n", row.name, row.person));
    }
    out
}
