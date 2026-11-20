//! `focus-group list|create|rename|add|remove` (0.46.17): the sidebar's
//! focus groups, as the shell CLI's `cmd_focus_group` answers them
//! (`cli/src/focus-group.sh`). The shell answers every form for now.

crate::route::shell_family!(&["focus-group", "focus-groups"]);
