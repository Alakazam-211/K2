#!/bin/sh
# agent.sh APALACHE_JAR OUT_DIR [MAX_STRING_LEN]: build the Jackson limits agent
# (JacksonLimits.java, README.md) against Apalache's own Jackson, and print the JVM option
# that loads it (`-javaagent:<jar>=MAX`), for the java command that starts an Apalache
# server. The jar is cached under OUT_DIR keyed on the Apalache jar's sha256, so another
# Apalache version builds its own. Needs a JDK (javac, jar). When it can't build, it prints
# nothing on stdout, one line on stderr saying the server keeps Jackson's 20,000,000-character
# limit, and exits 1.
set -e
JAR=${1:?usage: agent.sh APALACHE_JAR OUT_DIR [MAX_STRING_LEN]}
OUT=${2:?usage: agent.sh APALACHE_JAR OUT_DIR [MAX_STRING_LEN]}
MAX=${3:-200000000}
here=$(cd "$(dirname "$0")" && pwd)
fail() { echo "=== quint-jvm: the Jackson limits agent was not built ($1): the Apalache server keeps Jackson's 20,000,000-character limit" >&2; exit 1; }
[ -f "$JAR" ] || fail "no Apalache jar at '$JAR'"
for t in javac jar; do command -v $t >/dev/null 2>&1 || fail "no $t on PATH: a JDK is needed"; done
if command -v sha256sum >/dev/null 2>&1; then key=$(sha256sum "$JAR" | cut -c1-64)
elif command -v shasum >/dev/null 2>&1; then key=$(shasum -a 256 "$JAR" | cut -c1-64)
else fail "no sha256sum or shasum to key the cache"; fi
dir="$OUT/$key"
if [ ! -f "$dir/jackson-limits.jar" ]; then
  rm -rf "$dir.tmp"; mkdir -p "$dir.tmp/classes"
  javac -nowarn -cp "$JAR" -d "$dir.tmp/classes" "$here/JacksonLimits.java" >&2 || fail "javac failed against $JAR"
  printf 'Premain-Class: JacksonLimits\n' > "$dir.tmp/manifest.txt"
  jar cfm "$dir.tmp/jackson-limits.jar" "$dir.tmp/manifest.txt" -C "$dir.tmp/classes" . >&2 || fail "jar failed"
  rm -rf "$dir"; mv "$dir.tmp" "$dir"
fi
echo "-javaagent:$dir/jackson-limits.jar=$MAX"
