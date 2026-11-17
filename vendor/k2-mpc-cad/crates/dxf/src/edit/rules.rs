//! The edit rules: what the edit verbs may change, one function every verb asks before it plans
//! and the self-check asks again of what it wrote. Pure and bounded by the entity's pairs; the
//! same for a person and an agent (a hold refuses an agent only, and that is the kit's).
//!
//! In order: where the entity is (an external reference, a dynamic block's drawn form or
//! definition, any other block definition), what it is (its `edit` word: `1`, or a dynamic
//! block's reference, `dynblock`), an embedded object in it (a multi-line attribute), and the
//! layer it is on (an external reference's, or locked).

use k2_mpc_cad_model::{BlockKind, Doc};

use crate::error::{
    CadError, ATTRIB_MULTILINE, DYNAMIC_GEOMETRY, EXTERNAL_REFERENCE, HANDLES_OFF,
    IN_BLOCK_DEFINITION, LAYER_LOCKED, LAYER_MISSING, NOT_EDITABLE, PROXY,
};
use crate::mutant;
use crate::structure::Structure;

/// Whether entity `i` (of `doc`, read from `s`) may be changed by an edit verb (a move, a
/// property, an attribute value, a delete: each changes the entity as a whole).
pub fn entity(s: &Structure<'_>, doc: &Doc, i: usize) -> Result<(), CadError> {
    let e = &doc.entities[i];
    let addr = doc.entity_address(i);
    if let Some(o) = e.owner.map(|o| &doc.blocks[o]) {
        if o.xref || e.edit == "xref" {
            return Err(CadError::new(
                EXTERNAL_REFERENCE,
                format!("{addr} belongs to an external reference"),
                "edit the referenced drawing, then reload it in the app",
            ));
        }
        let owner = e.owner.unwrap_or(0);
        let dynamic =
            o.dynamic_of.is_some() || doc.blocks.iter().any(|b| b.dynamic_of == Some(owner));
        if dynamic {
            return Err(CadError::new(
                DYNAMIC_GEOMETRY,
                format!("{addr} is inside a dynamic block's definition or drawn form"),
                "change the block's parameters in the app",
            ));
        }
        if o.kind == BlockKind::Block {
            return Err(CadError::new(
                IN_BLOCK_DEFINITION,
                format!(
                    "{addr} is inside a block definition: a change there changes every reference"
                ),
                "edit the block in the app's block editor",
            ));
        }
    }
    let dynblock_ok = !mutant::active("DynblockAttribRefused");
    match e.edit {
        "1" => {}
        "dynblock" if dynblock_ok && &*e.kind == "INSERT" => {}
        "proxy" if !mutant::active("ProxyWritten") => {
            return Err(CadError::new(
                PROXY,
                format!("{addr} is another application's object (a proxy)"),
                "edit it in the application that made it",
            ))
        }
        "minted" => {
            return Err(CadError::new(
                HANDLES_OFF,
                format!("{addr} has no handle"),
                "save the drawing again in the app (it gives every object a handle)",
            ))
        }
        "xref" => {
            return Err(CadError::new(
                EXTERNAL_REFERENCE,
                format!("{addr} belongs to an external reference"),
                "edit the referenced drawing, then reload it in the app",
            ))
        }
        why => {
            return Err(CadError::new(
                NOT_EDITABLE,
                format!("{addr} isn't changed by the edit verbs ({why})"),
                format!("`entity props {addr}` says why; change it in the app"),
            ))
        }
    }
    // an embedded object (2018): a multi-line attribute, or a multiline text's columns
    let eu = &s.entities[i];
    for (k, u) in std::iter::once(&eu.head).chain(&eu.followers).enumerate() {
        if (u.first..u.end).any(|p| s.tok.pairs[p].code == 101) {
            return Err(if k > 0 && u.kind == "ATTRIB" {
                CadError::new(
                    ATTRIB_MULTILINE,
                    format!("{addr} has a multi-line attribute"),
                    "edit it in the app",
                )
            } else {
                CadError::new(
                    NOT_EDITABLE,
                    format!("{addr} isn't changed by the edit verbs (embedded)"),
                    format!("`entity props {addr}` says why; change it in the app"),
                )
            });
        }
    }
    if let Some(l) = doc.layer_named(&e.layer).map(|l| &doc.layers[l]) {
        if l.xref {
            return Err(CadError::new(
                EXTERNAL_REFERENCE,
                format!("{addr} is on an external reference's layer"),
                "edit the referenced drawing, then reload it in the app",
            ));
        }
        let locked = if mutant::active("LockedReadAsFrozen") {
            l.locked || l.frozen
        } else {
            l.locked
        };
        if locked {
            return Err(CadError::new(
                LAYER_LOCKED,
                format!("{addr} is on a locked layer"),
                "unlock the layer in the app, then retry",
            ));
        }
    }
    Ok(())
}

/// The layer named `name` as a place to put an entity: it must exist, not belong to an external
/// reference and not be locked. Its index in `doc.layers`.
pub fn target_layer(doc: &Doc, name: &str) -> Result<usize, CadError> {
    let Some(i) = doc.layer_named(name) else {
        return Err(CadError::new(
            LAYER_MISSING,
            "the drawing has no layer of that name",
            "`layer list` lists the drawing's layers",
        ));
    };
    let l = &doc.layers[i];
    if l.xref {
        return Err(CadError::new(
            EXTERNAL_REFERENCE,
            "that layer belongs to an external reference",
            "put the entity on one of this drawing's own layers",
        ));
    }
    if l.locked {
        return Err(CadError::new(
            LAYER_LOCKED,
            "that layer is locked",
            "unlock the layer in the app, then retry",
        ));
    }
    Ok(i)
}
