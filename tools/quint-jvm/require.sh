# require.sh (sourced): `qjvm_require WHERE`, the guard every Apalache server a Quint job starts
# goes through (tools/undo/job.sh's export check, tools/undo/tlc.sh). Apalache reads a request's
# configuration, which carries the whole Quint IR, as one JSON string of at most 20,000,000
# characters (Jackson's default); the undo model's IR is past it (docs/MODEL-CHECK.md, "The
# model's size"), so the Jackson limits agent (agent.sh) is required. Without it in
# APA_JAVA_OPTS (agent.sh couldn't build it, or the variable was set to something else) the job
# stops here, in one line that names the limit and the model's IR size, before any request
# could fail inside Jackson. The size is `quint compile --target json` of the instance named by
# QJVM_IR_MAIN in QJVM_IR_FILE (default: the export check's, q_export_check.qnt); compiling to
# JSON runs in quint itself, not in Apalache.
qjvm_require() {
  case "${APA_JAVA_OPTS:-}" in
    *jackson-limits.jar*) return 0 ;;
  esac
  f=${QJVM_IR_FILE:-q_export_check.qnt}; m=${QJVM_IR_MAIN:-q_export_check}
  # to a file: node cuts stdout into a pipe short at exit (64 KiB)
  ir=""; quint compile --target json --main="$m" "$f" > "qjvm-ir-$m.json" 2>/dev/null && ir=$(wc -c < "qjvm-ir-$m.json" | tr -d ' ')
  # in a job with row verdicts (tools/undo/expect.sh) the failure is one of its problems, so the
  # job's `=== EXPECT: RED` lists it; the exit below then ends the job through expect's trap
  if command -v _expect_fail >/dev/null 2>&1; then
    _expect_fail "quint-jvm: $1: no Jackson limits agent; the model's IR (${ir:-unknown} bytes) is read against Jackson's 20,000,000-character limit"
  fi
  echo "=== quint-jvm: $1: no Jackson limits agent in APA_JAVA_OPTS ('${APA_JAVA_OPTS:-}'). Apalache reads the model's IR as one JSON string of at most 20,000,000 characters (Jackson's default limit); this model's IR ($m) is ${ir:-unknown} bytes. Stopping the job before any Apalache request (tools/quint-jvm/README.md)."
  exit 1
}
