    {
      "name": "skills",
      "usage": "k2 skills <list|create|remove|profile|...> [...]",
      "description": "Manage skill profiles (SKILL.md) for workspaces/agents.",
      "args": [],
      "flags": []
    },
    {
      "name": "settings",
      "usage": "k2 settings [--mode <off|custom|manager|k2>] | k2 settings set <key> <true|false>",
      "description": "Show this workspace's settings (name, mode, heartbeat, pinned), or set its mode with --mode off|custom|manager|k2. set: the owner turns one server preview switch on or off: voiceRoomsPreview, voiceTunnelPreview, computePreview, ciPreview, boardFilePreview, videoRoomsPreview, usageSingleRecord, agentProcessNames or homeChat (any other key exits 2 with that list; agents, Admins and Members are refused, exit 3). A daemon refusal exits 3.",
      "args": [],
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

