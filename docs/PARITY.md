# Agent parity

Everything a person can do on a board, and how an agent does it, at file level with no live session. `tests/parity.rs` runs each verb row the person's way, as the raw ops a canvas sends, and the agent's way, and the boards must match. This file is generated from `crates/board/src/parity.rs`. The list of actions is Board's own: portions derived from Board, Copyright (c) 2026 Baden Ballard, MIT; see `THIRD-PARTY-NOTICES.md`.

| Action | A person | An agent |
|---|---|---|
| `add-sticky` | Add a sticky note | `add` |
| `add-text` | Add a text block | `add` |
| `add-shape` | Add a shape | `add` |
| `draw-pen` | Draw with the pen or highlighter | `add` |
| `add-frame` | Add a frame | `add` |
| `add-card` | Add a card | `add` |
| `add-table` | Add a table | `add` |
| `add-mind-node` | Add a mind-map node | `add` |
| `add-image` | Add a picture | `add` |
| `paste-text` | Paste text as stickies or a text block | `add`, `apply` |
| `edit-fields` | Edit text, colours, sizes, alignment, tags, links, crops and card fields | `update` |
| `resize` | Resize | `update` |
| `rotate` | Rotate | `update` |
| `move-objects` | Move or nudge, with groups and frame contents | `move` |
| `draw-connector` | Draw a connector | `connect` |
| `reattach-connector` | Drag a connector end to another object | `update` |
| `delete` | Delete, with what hangs off the object | `delete` |
| `erase-stroke` | Erase a pen stroke | `delete` |
| `cut` | Cut | `delete`, `list` |
| `copy` | Copy and paste | `list`, `paste` |
| `duplicate` | Duplicate | `duplicate` |
| `connected-copy` | Add a connected copy from a connection dot | `connected-copy` |
| `copy-style` | Copy style | `copy-style` |
| `tidy` | Tidy up | `tidy` |
| `align` | Align | `align` |
| `distribute` | Distribute | `distribute` |
| `group` | Group | `group` |
| `ungroup` | Ungroup | `ungroup` |
| `lock` | Lock | `lock` |
| `unlock` | Unlock | `lock` |
| `bring-to-front` | Bring to front | `order` |
| `send-to-back` | Send to back | `order` |
| `bring-forward` | Bring forward | `order` |
| `send-backward` | Send backward | `order` |
| `reorder-frames` | Reorder frames for presenting | `frame-order` |
| `comment` | Add a comment, pinned or free | `comment` |
| `reply` | Reply to a comment | `reply` |
| `resolve` | Resolve or reopen a comment | `resolve` |
| `delete-comment` | Delete a comment | `delete` |
| `react` | Add or take off a reaction | `react` |
| `stamp` | Stamp an object, as a reaction drawn big | `react` |
| `vote` | Vote | `vote` |
| `take-back-vote` | Take back a vote | `vote` |
| `start-voting` | Start or stop voting | `meta` |
| `rename-board` | Change the board title | `meta` |
| `add-check` | Add a check item | `check-add` |
| `tick-check` | Tick or edit a check item | `check-set` |
| `remove-check` | Remove a check item | `check-remove` |
| `edit-cell` | Edit a table cell | `table-cell` |
| `add-row` | Add a table row | `table-add-row` |
| `remove-row` | Remove a table row | `table-remove-row` |
| `add-column` | Add a table column | `table-add-column` |
| `remove-column` | Remove a table column | `table-remove-column` |
| `mind-child` | Add a mind-map child | `mind-child` |
| `mind-sibling` | Add a mind-map sibling | `mind-child` |
| `use-template` | Add a template | `templates`, `template` |
| `search` | Search the board | `search` |
| `read-board` | Look at the board and read an object | `list`, `read`, `fields`, `references` |
| `see-author` | See who made an object | `read` |
| `undo` | Undo | `inverse` |
| `redo` | Redo | `inverse` |
| `restore-deleted` | Undo a delete | `put-back` |
| `clear-missing-picture` | Clear a picture whose bytes are missing | `clear-blob` |
| `paste-many` | Paste or draw several things as one step | `apply` |
| `view` | Pan, zoom, fit, minimap, present | None needed. It changes nothing in the file. |
| `select` | Select, marquee | None needed. It changes nothing in the file. A verb names its objects. Showing a selection to others is live. |
| `snap-guides` | Smart guides and grid snap | None needed. They pick numbers for a person dragging. A verb gives the numbers. |
| `appearance` | Light and dark | None needed. It changes nothing in the file. |
| `timer` | Run a timer | Not in a file. A running timer is out of the file by design: its end time is stale once sent. It's the host's. |
| `history` | Board history | Not in a file. A file holds no history. Version control or the live session holds it. |
| `export` | Export PNG, PDF | The host's. It needs a renderer, so it's the host's. The file itself is the JSON export. |
| `open-link` | Open a link on an object | The host's. Opening anything is the host's. `read` gives the address. |
| `access` | Who can open a board, projects, trash | The host's. Access and places are the host's: a file carries neither. |
| `save` | Save | The host's. The host owns the file, through `BoardFile`, with the guard. |
| `presence` | See who's here, their cursors, follow them | With live. One writer at a time, with no presence. It comes with live. |
