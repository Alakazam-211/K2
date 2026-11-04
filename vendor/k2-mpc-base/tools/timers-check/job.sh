# The timers check (tools/README.md, "Timers check"): a commands file for tools/job.sh.
#
#   sh tools/job.sh tools/timers-check/job.sh
#
# Builds tools/timers-check against the kit with no features (a host's build), debug and
# release, and runs both: the timers are there, off by default, and count a session's calls
# once switched on; the release run prints what a scope costs on the node (`calibrate`).
cp Cargo.lock tools/timers-check/Cargo.lock
timed timers_check_debug cargo run --manifest-path tools/timers-check/Cargo.toml
timed timers_check_release cargo run --release --manifest-path tools/timers-check/Cargo.toml
