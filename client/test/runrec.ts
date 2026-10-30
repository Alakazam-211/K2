// One run record per suite run, in the kit's evidence format (tools/evidence/README.md): a line
// `RUNREC {json}` on stdout, which tools/evidence/collect.py takes from a node job's log so the
// evidence notes can cite these counts (`[run JOB/RUN:field]`). Test-side only.

/** Print the record for run `run` (unique in the job), its counts as `fields`. */
export function runrec(run: string, fields: Record<string, number>, status = "COMPLETE"): void {
  console.log(`RUNREC ${JSON.stringify({ job: null, run, tool: "client", status, ...fields })}`);
}
