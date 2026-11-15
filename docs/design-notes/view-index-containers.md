# The view index's containers: the tree, for an outliner

What the kit gives a view about containers (a deck's slides, a scene's collections, a
timeline's tracks), so an outliner panel draws them from the view index alone, with no pack code
and no read of the doc's keyed state.

## Derived values: `Container::derived`

Small texts only the pack works out for a container, such as a timeline track's label and kind:
the view schema's `container_derived` names them, each container carries its own as `derived`
(`{name: text}`, absent when it has none), and each change frame's
`derived.containers.<container>.<name>` updates them (`null` removes one). The client library's
`KeyedWatch.containerDerived(name, container)` reads one.

## The tree: `Container::parent`

Each container has `parent`, the address of the container it sits inside, or none at the top
level (`crates/base/src/view_index.rs`, `Container::parent`; on the wire, a snapshot container's
`parent`, absent at the top level: `client/WIRE.md`, "Containers in containers").

- **Draw the tree from `parent`.** The containers come in one flat list, in `position` order; a
  container's children are the containers whose `parent` is its address, in that same order.
  The flat order doesn't change with nesting: a collection's children need not follow it.
- **The kit has checked it.** Every `parent` names another container of the same view; there
  is no cycle; no container sits deeper than `CONTAINER_DEPTH_MAX` levels (32, the top level
  being 1). A parent the file gets wrong is left out, so that container shows at the top level.
  A view can walk the tree without guarding against loops.
- **Keeping it current.** A change frame's container delta carries `parent_after` (the new
  parent's address, or `null` for the top level) whenever a container comes or its parent
  changes, including a `props` delta naming no field when only the parent moved. The client
  library's `KeyedView` applies it; a view reads `view.containers[i].parent`.
- **Unit nesting is separate.** A unit inside a unit (a group's shapes, a collection's objects)
  is the unit's own `parent`; the schema's `nesting` declares which unit kinds nest. A container
  tree and unit nesting can both be present.
