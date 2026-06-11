# Cursor IDE Chat Migration

K2SO can migrate conversations from the Cursor IDE to the Cursor CLI format, allowing you to resume IDE-created conversations from the terminal using `cursor-agent --resume`.

## How It Works

### Cursor's Two Storage Systems

Cursor stores conversations in two separate systems that don't share data:

| System | Location | Used By |
|--------|----------|---------|
| **IDE (Composer)** | `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb` | Cursor IDE |
| **CLI (Agent)** | `~/.cursor/chats/{md5_hash}/{session_uuid}/store.db` | `cursor-agent` CLI |

The IDE stores conversations as JSON in a SQLite key-value store (`cursorDiskKV` table). The CLI stores them as content-addressed protobuf blobs in per-session SQLite databases.

When you create a conversation in the Cursor IDE, it is **not** available to `cursor-agent --resume`. K2SO bridges this gap.

### Storage Format Details

#### CLI Session Format (`~/.cursor/chats/`)

```
~/.cursor/chats/
  {MD5(absolute_project_path)}/     # e.g. c3e8d05ad823ab6c...
    {session_uuid}/
      store.db                       # SQLite database
        Table: meta
          key "0" -> hex-encoded compact JSON:
            {"agentId":"uuid","latestRootBlobId":"sha256","name":"...","mode":"agent","createdAt":1234567890,"lastUsedModel":"composer-2-fast"}
        Table: blobs
          id (TEXT) -> SHA-256 hash of data
          data (BLOB) -> protobuf or JSON content
```

The `blobs` table uses content-addressed storage: each blob's ID is the SHA-256 hash of its data. The conversation is a Merkle tree:

- **Root blob**: protobuf containing hash references to child blobs + workspace metadata
- **Message blobs**: JSON `{"role":"user"|"assistant","content":[{"type":"text","text":"..."}]}`
- **Context blobs**: protobuf-wrapped file contents from tool reads
- **Summary blobs**: conversation checkpoints

Hash references appear as protobuf length-delimited fields with exactly 32 bytes (the SHA-256 hash). They can appear in multiple protobuf field numbers (1, 3, 8, 13, etc.).

#### IDE Session Format (`globalStorage/state.vscdb`)

Conversations are indexed in workspace-specific databases:
```
~/Library/Application Support/Cursor/User/workspaceStorage/{hash}/state.vscdb
  Table: ItemTable
    key "composer.composerData" -> JSON with allComposers[] array
```

Each composer has metadata (name, timestamps, mode) and a `conversationState` field containing the root blob data. This field is encoded as either:
- **Base64** (prefixed with `~`): newer sessions
- **Hex**: older sessions

Individual message blobs are stored globally:
```
~/Library/Application Support/Cursor/User/globalStorage/state.vscdb
  Table: cursorDiskKV
    key "composerData:{composerId}" -> full session metadata JSON
    key "agentKv:blob:{sha256_hash}" -> blob data (same content-addressed format as CLI)
```

### Migration Process

1. **Discover**: Scan `workspaceStorage/*/workspace.json` to find workspaces matching the project path. Read `composer.composerData` from each workspace's `state.vscdb` to enumerate IDE sessions.

2. **Check migratability**: A session is migratable only if its `composerData` in `globalStorage` has a non-empty `conversationState` field (length > 10 characters). Sessions created in "chat" mode (not "agent" mode) may lack this field.

3. **Decode root blob**: Parse `conversationState` — detect format by checking if it starts with `~` (base64) or is hex-encoded. Decode to raw protobuf bytes.

4. **Collect blob references**: Parse the root blob's protobuf structure to find ALL 32-byte hash references across all field numbers. These reference the conversation's message blobs, context blobs, and summary blobs.

5. **Recursively copy blobs**: For each referenced hash, read the blob from `globalStorage` (`agentKv:blob:{hash}`), write it to the new `store.db`, then scan THAT blob for additional hash references. Continue until all referenced blobs are copied. This handles arbitrary nesting depth.

6. **Write metadata**: Create the `meta` table entry with compact JSON (no spaces, matching cursor-agent's native format) containing `agentId`, `latestRootBlobId`, `name`, `mode`, `createdAt`, and `lastUsedModel`.

7. **Create directory**: Place the `store.db` at `~/.cursor/chats/{MD5(project_path)}/{composerId}/store.db`.

### Important Details

- **Hash directory**: The directory name under `~/.cursor/chats/` is the MD5 hash of the absolute project path (e.g., MD5 of `/Users/you/projects/myapp`). `cursor-agent` scans all hash directories when resuming by UUID.

- **Session ID conflict**: If the `composerId` exists in `globalStorage`'s `composerData`, `cursor-agent --resume` will load from `globalStorage` instead of `store.db`. Since the `globalStorage` entry has `fullConversation: []` (empty), the conversation appears blank. The migrated `store.db` has the full blob tree, but cursor-agent prioritizes `globalStorage`. This is a known limitation — the conversation context IS loaded (visible in the context usage percentage) but history is not displayed.

- **Blob integrity**: All blobs are content-addressed with SHA-256. After migration, you can verify integrity by checking that each blob's ID matches `SHA-256(data)`.

- **Non-migratable sessions**: Sessions created in Cursor's "chat" mode (not "agent" mode) store conversations differently and don't have a `conversationState` field. These cannot be migrated.

## User-Facing Flow in K2SO

1. When a workspace is opened, K2SO checks for unmigrated Cursor IDE sessions
2. If migratable sessions are found, a toast notification appears: "X Cursor conversations found — Migrate"
3. Clicking the toast opens the workspace settings page
4. The **Cursor IDE Conversations** panel shows all sessions with their migration status
5. Clicking **Migrate** processes sessions one at a time with visual progress
6. Migrated sessions appear in the Chat History sidebar and can be resumed via terminal

## Limitations

- Conversations created in Cursor's "chat" mode cannot be migrated (no `conversationState`)
- Resumed migrated sessions show context usage but may not display visual message history due to cursor-agent prioritizing `globalStorage` data
- Migration is read-only — it does not modify any Cursor IDE data
