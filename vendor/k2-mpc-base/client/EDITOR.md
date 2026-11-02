# The live editor: moved to the host

Since kit v0.10.2 the client has no editor component (`k2-mpc-client/editor`, `mountLiveEditor`
and the CodeMirror binding `codemirrorLive` are gone). The host draws its own editor UI over the
library: see [HOST.md](HOST.md) for the calls an editor makes, the rules it keeps and the tests
that cover them, and [PRESENCE.md](PRESENCE.md) for carets and rejected changes as data. This
file stays as a pointer; the component's last documentation is in the kit's history (v0.10.1).
