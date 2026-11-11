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

Bump `version` on any change. Placeholders are `{{name}}`, `{{person}}` and
the template's declared `[params]`; a line whose placeholder is empty is
dropped. `k2 agent template show <id> --files` prints what a template holds.
