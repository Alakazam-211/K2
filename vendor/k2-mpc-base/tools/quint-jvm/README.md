# quint-jvm: JVM options for the Apalache server

Apalache 0.62 reads a remote request's configuration (quint's `compile --target tlaplus`
and `verify` send the whole Quint IR in it, as one JSON string) with a plain Jackson
`JsonFactory`, so Jackson 2.x's default `StreamReadConstraints` apply: a string of at most
20,000,000 characters. The undo model's IR with the export check's step and invariants is
just about that size, and past it the server answers `Invalid strict JSON: String value
length (...) exceeds the maximum allowed (20000000 ...)`. Jackson 2.x reads no system
property for the limit, so the server is started with a small Java agent that raises the
default before Apalache runs:

| File | Does |
|---|---|
| `JacksonLimits.java` | the agent: `StreamReadConstraints.overrideDefaultStreamReadConstraints` with `maxStringLength` raised (default 200,000,000); every other limit as Jackson's defaults |
| `agent.sh APALACHE_JAR OUT_DIR [MAX]` | builds the agent against Apalache's own Jackson (needs `javac` and `jar`), cached under `OUT_DIR/<sha256 of the Apalache jar>/` so an Apalache version bump rebuilds it, and prints the JVM option; when it can't build, one `=== quint-jvm:` line says why and that the server keeps the 20,000,000 limit |

`tools/undo/job.sh` builds it once per job (into `qjvm/`, exported as `APA_JAVA_OPTS`), and
both servers it starts use it: the export check's (`tools/quint/`, unchanged) and
`tools/undo/tlc.sh`'s. Only the size limit changes: the TLA+ an export produces is the same
with and without the agent. A server quint starts on its own keeps the default.

**The agent is required** (since 2026-11-04; the undo model's IR is 21.3 MB, past the limit
on every instance). `require.sh`'s `qjvm_require` runs before each Apalache server a job
starts: the export check's (`tools/undo/job.sh`) and `tools/undo/tlc.sh`'s. Without the agent
in `APA_JAVA_OPTS` (`agent.sh` couldn't build it, or the variable was set to something else)
the job stops there with one line naming the limit and the model's IR size:

    === quint-jvm: <server>: no Jackson limits agent in APA_JAVA_OPTS ('...'). Apalache reads
    the model's IR as one JSON string of at most 20,000,000 characters (Jackson's default
    limit); this model's IR (q_export_check) is N bytes. Stopping the job before any Apalache
    request (tools/quint-jvm/README.md).

A server quint starts on its own keeps Jackson's default, so no compile of the model may reach
one: the job fetches Apalache with a one-line module (`q_fetch`), and stops if its own server
didn't come up rather than let a compile start one. `quint run`, `typecheck` and `compile
--target json` don't use Apalache. If the IR is past the agent's limit too, the export check
and `tlc.sh`'s compile say so in one line instead of retrying a tool error that can't pass.
