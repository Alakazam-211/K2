//! `k5 doc` (also `docs`): the shell CLI answers it. Its lane makes it
//! native here, and only here: the family's words, owner, verb rows and
//! `run` live in this module, and `route::TABLE` reads them.

crate::route::shell_family!(&["doc", "docs"]);
