//! Examples for the wire types S12 adds (prd-sew-12 §7).

use super::{ID_CATEGORY, ID_CHANNEL, ID_ROLE, ID_SPACE, overwrite, role_min};
use crate::api::moderation::*;
use crate::api::overwrites::*;
use crate::api::roles::*;
use crate::api::spaces::*;
use crate::gateway::events::*;
use crate::http::{Page, Patch};
use crate::perms::Perms;
use crate::policy::Policies;
use crate::registry::Example;

impl Example for CreateRoleRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                CreateRoleRequest {
                    name: "Moderators".into(),
                    color: Some(3_447_003),
                    perms: Some(Perms::MANAGE_MESSAGES),
                    mentionable: Some(true),
                    hoist: Some(true),
                    position: Some(2),
                },
            ),
            (
                "min",
                CreateRoleRequest {
                    name: "Helpers".into(),
                    color: None,
                    perms: None,
                    mentionable: None,
                    hoist: None,
                    position: None,
                },
            ),
        ]
    }
}

impl Example for UpdateRoleRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                UpdateRoleRequest {
                    name: Some("Mods".into()),
                    color: Patch::Set(0x3b_82_f6),
                    perms: Some(Perms::MANAGE_MESSAGES | Perms::PIN_MESSAGES),
                    mentionable: Some(false),
                    hoist: Some(true),
                },
            ),
            (
                "min",
                UpdateRoleRequest {
                    color: Patch::Clear,
                    ..UpdateRoleRequest::default()
                },
            ),
        ]
    }
}

impl Example for ReorderRolesRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            ReorderRolesRequest {
                role_ids: vec![ID_ROLE, super::id(17)],
            },
        )]
    }
}

impl Example for MemberRolesRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            MemberRolesRequest {
                role_ids: vec![ID_ROLE],
            },
        )]
    }
}

impl Example for Page<Role> {
    fn examples() -> Vec<(&'static str, Self)> {
        let mods = Role {
            id: ID_ROLE,
            space_id: ID_SPACE,
            name: "Moderators".into(),
            color: Some(0x3b_82_f6),
            position: 1,
            perms: Perms::MANAGE_MESSAGES,
            mentionable: true,
            is_everyone: false,
            hoist: true,
        };
        vec![(
            "full",
            Page {
                items: vec![role_min(), mods],
                has_more: false,
            },
        )]
    }
}

impl Example for SetOverwriteRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            SetOverwriteRequest {
                target_kind: OverwriteTarget::Role,
                target_id: ID_ROLE,
                allow: Perms::PIN_MESSAGES,
                deny: Perms::ADD_REACTIONS,
            },
        )]
    }
}

impl Example for OverwriteRef {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            OverwriteRef {
                target_kind: OverwriteTarget::Member,
                target_id: super::ID_BOB,
            },
        )]
    }
}

impl Example for ChannelOverwrites {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![
            (
                "full",
                ChannelOverwrites {
                    channel_id: ID_CHANNEL,
                    space_id: ID_SPACE,
                    category_id: Some(ID_CATEGORY),
                    synced: false,
                    overwrites: vec![overwrite()],
                },
            ),
            (
                "min",
                ChannelOverwrites {
                    channel_id: ID_CHANNEL,
                    space_id: ID_SPACE,
                    category_id: None,
                    synced: false,
                    overwrites: vec![],
                },
            ),
        ]
    }
}

impl Example for TimeoutRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            TimeoutRequest {
                duration_ms: 600_000,
            },
        )]
    }
}

impl Example for SlowModeRequest {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![("full", SlowModeRequest { slow_mode_s: 30 })]
    }
}

impl Example for RoleDeleted {
    fn examples() -> Vec<(&'static str, Self)> {
        vec![(
            "full",
            RoleDeleted {
                space_id: ID_SPACE,
                role_id: ID_ROLE,
            },
        )]
    }
}

impl Example for InstallUpdated {
    fn examples() -> Vec<(&'static str, Self)> {
        let p = Policies {
            gifs_enabled: true,
            ..Policies::default()
        };
        vec![(
            "full",
            InstallUpdated {
                policy_digest: p.digest(),
            },
        )]
    }
}
