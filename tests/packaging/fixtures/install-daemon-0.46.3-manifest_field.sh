# Frozen copy of `manifest_field` from scripts/install-daemon.sh at tag
# v0.46.3 (`git show v0.46.3:scripts/install-daemon.sh`). Fleet boxes and
# provision scripts still run this parser against every new
# daemon-latest.json, so tests/packaging/daemon-manifest.sh runs it against
# a manifest that carries App engine entries (update continuity A1/T29).
# Never edit: it is what is in the field.
manifest_field() {
    _json="$1"; _key="$2"; _field="$3"
    _obj=$(printf '%s' "$_json" | tr '\n' ' ' \
        | sed -n "s/.*\"${_key}\"[[:space:]]*:[[:space:]]*{\([^}]*\)}.*/\1/p")
    [ -n "$_obj" ] || return 1
    printf '%s' "$_obj" \
        | sed -n "s/.*\"${_field}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p"
}
