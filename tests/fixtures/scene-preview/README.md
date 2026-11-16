# Scene preview fixture

A live BLEND scene's previews as the host draws and serves them, for the
client's tests: check that an object's box (its listed `bounds` through its
transform, projected with the image's `camera`) lines up with the object
drawn, and pick objects with the id image.

The scene is a furnished living room (Z up, metres): the units of the
living-room sample snapshot that draw something, named as the file names
them, with their transforms composed through their parents (the books on
the shelf, the vase on the table, the clock hand on the clock, an empty).
The snapshot has no geometry, so each mesh is a stand-in made of boxes,
shared by every object that uses it (the four chairs, the two books). Empties
and lights are not drawn and the hidden left wall is left out. The sofa is
drawn without its subdivision modifier, so its listed box says `approx`
(`approx_reason: "modifiers"`) and the scene and camera views say
`without_modifiers`. The file's two cameras are 16:9.

Addresses are the live keys, as a BLEND read gives them: each object and
camera carries its `key` in the K2SCENE1 (`@obj:N` from the sample
snapshot's unit keys; a camera `@cam:N` from its camera object's number),
and the listing, the route and the image answers name each view by it
(`@obj:15` is the sofa, `@cam:1` the active camera); the scene view is
`@scene:main`. Every address is a native address in the multiplayer kit's
grammar, so it is also a presence view address. An object's name is its
container's `title` and in the image's `ids` table.

Each view is drawn at the `thumb2x` width (640 px), transparent, wide
(`shape=wide`), with its id image, through the daemon's own path: the
scene renderer's request, the worker, the cache, and the preview route's
answers.

| File | What |
|---|---|
| `living-room.k2scene` | the scene as K2SCENE1 (the header is JSON after the 8-byte magic and a little-endian u32 length: each object's `id`, `key` and world `transform`, column-major) |
| `listing.json` | the route's listing (`look=transparent`, `shape=wide`): each container's `key`, `render_hash`, and an object's local `bounds` |
| `scene.webp`, `scene.ids.png`, `scene.json` | `@scene:main`, the auto view fitted to the contents: the still, its id image, and the route's answer for the still |
| `cam-Camera.webp`, `cam-Camera.ids.png`, `cam-Camera.json` | `@cam:1`, the file's active camera, `Camera` (its own 16:9) |
| `obj-Sofa.webp`, `obj-Sofa.ids.png`, `obj-Sofa.json` | `@obj:15`, the sofa on its own |

The JSON files are the route's answers pretty-printed, values unchanged; an
image's answer leaves out its `base64` (the still is its own file). Each
names the image's `size`, `shape`, the `camera` that drew it (`view`, `proj`
and the mapping to pixels in `k2_scenerender::proto::CameraMeta`) and its
`ids` table. An id image is an 8-bit RGB PNG at the still's size: each pixel
`r << 16 | g << 8 | b` is 0 where nothing is drawn, else the index + 1 of the
object under it, which `ids.objects` names.

Fixed so the files never drift: the render hashes are keyed with a fixed
test key (a host keys them with the space's own), and `render_ms` and
`rendered_at_ms` are 0.

## Checked and regenerated

`docs::preview::scene_fixture_tests` in `k2-daemon` makes every file again in
memory and asserts each is byte for byte the one here (the renderer gives the
same bytes on every CPU), and checks that every id image pixel of an object
falls inside that object's projected box. To write them again after a
renderer or route change:

```sh
K2_REGEN_FIXTURES=1 cargo test -p k2-daemon --lib docs::preview::scene_fixture_tests
```

then run it again without the variable and commit the files with the change.
