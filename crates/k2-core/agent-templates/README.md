# Built-in agent templates

Each folder here is one built-in agent template (prd-agent-templates-v1 §2).
`build.rs` embeds every folder, so adding a template is a folder with no Rust
edit. Folders starting with `_` are not templates: `_shared/` holds the
shared "Working with your person and the team" block that `shared_block = true`
appends to the charter. This README is not embedded.

A template folder may hold only:

    template.toml          id, version, what the agent gets
    ROLE.md                the charter (the K2:MANAGED block of the agent's ROLE.md)
    wiki/<Note>.md         starter notes (headings only; the agent fills them)
    context/<label>.md     the template's own always-on layers
    skills/<name>/SKILL.md

Bump `version` on any change. Placeholders are `{{name}}`, `{{person}}`,
`{{open_questions}}` and the template's declared `[params]`; a line whose
placeholder is empty is dropped. `k2 agent template show <id> --files`
prints what a template holds.

First-conversation questions (at most 8, in the order the agent asks):

    [[questions]]
    id = "audience"                      # a-z, 0-9, _; unique
    ask = "Who do you write for?"        # one plain line, no placeholders
    fills = "role"                       # or "wiki:<Note>#<Heading>"

A hirer may answer some at hire (`--answer audience=…`, `answers` in the
body). A `role` answer goes in ROLE.md's "What I know about my work"; a
`wiki:` answer goes under that heading of that starter note (one the
template ships), or into ROLE.md when the note isn't written. A template
with questions also gets `_shared/first-conversation.md` plus the list of
questions still open, and `{{open_questions}}` (for `[hello] brief`) is
those asks on one line, empty when none are open.
