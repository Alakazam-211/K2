    {
      "name": "skills",
      "usage": "k2 skills <list|create|remove|profile|...> [...]",
      "description": "Manage skill profiles (SKILL.md) for workspaces/agents.",
      "args": [],
      "flags": []
    },
    {
      "name": "settings",
      "usage": "k2 settings [--mode <off|custom|manager|k2>]",
      "description": "Show this workspace's settings (name, mode, heartbeat, pinned), or set its mode with --mode.",
      "args": [],
      "flags": [
        {"name": "--mode", "arg": "mode", "description": "off | custom | manager | k2: set this workspace's agent mode (POST /cli/mode)."}
      ]
    },
    {
      "name": "settings set",
      "usage": "k2 settings set <key> <true|false>",
      "description": "The owner turns one server preview switch on or off (POST /cli/settings/update): voiceRoomsPreview, voiceTunnelPreview, computePreview, ciPreview, boardFilePreview, videoRoomsPreview, usageSingleRecord, agentProcessNames, homeChat or homeChatPush (any other key exits 2 with that list; agents, Admins and Members are refused, exit 3; an agent's ciPreview true, with permission cards on, ends with the k2 permission request ci.preview line to ask the owner). A daemon refusal exits 3.",
      "args": [{"name": "key", "required": true}, {"name": "value", "required": true, "description": "true or false (on and off work too)."}],
      "flags": []
    },
    {
      "name": "version",
      "usage": "k2 version",
      "description": "Print the CLI version.",
      "args": [],
      "flags": []
    },
    {
      "name": "cli status",
      "usage": "k2 cli status [--json]",
      "description": "Answered by the k5 binary only (the shell CLI says so and exits 2): which command families the binary answers itself (native), from their --schema entry (spec) or hands to the shell CLI (bash), and its version. For the owner, how many calls of each family fell through to the shell CLI (GET /cli/cli-usage); with a passport, or the daemon down, the table alone and one line on stderr. Exit 0.",
      "args": [],
      "flags": [
        {"name": "--json", "arg": null, "description": "One JSON document: {version, native, spec, bash, counts (shown, owner-only or unavailable), families: [{family, words, owner, fallthroughs, verbs}]}."}
      ]
    },
    {
      "name": "--schema",
      "usage": "k2 --schema [json]",
      "description": "Print this command-surface manifest as JSON (one call to load the whole CLI surface).",
      "args": [],
      "flags": []
    },
    {
      "name": "study",
      "usage": "k2 study [list|<topic>|--json]",
      "description": "Bounded teaching pages (not --help). Catalog, or one topic page. Daemon-optional. License, people lists, errors.",
      "args": [{"name": "topic", "required": false, "description": "list, or a topic id: what, source, map, identity, send, human, people, auth, errors, context, db, mail, connect-boundary, skins, feedback-loop, ticket-brief, notifications."}],
      "flags": [
        {"name": "--json", "arg": null, "description": "Machine-readable: catalog {topics:[{id,title}]} or page {id,body}."}
      ]
    },
    {
      "name": "help",
      "usage": "k2 help [<verb>|--advanced|--internal]",
      "description": "Human-readable help. 'k2 help <verb>' shows full prose for one verb.",
      "args": [{"name": "verb", "required": false}],
      "flags": [
        {"name": "--advanced", "arg": null, "description": "Power-user verbs."},
        {"name": "--internal", "arg": null, "description": "Orchestrator-RPC verbs."}
      ]
    }
  ]
}
EOF
}

