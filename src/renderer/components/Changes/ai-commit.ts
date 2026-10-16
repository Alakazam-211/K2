// "AI Commit" / "AI Commit & Merge" prompt — shared by the Changes tab and
// the Changes drawer (ChangesPanel). The agent runs `git diff` itself; the
// prompt only names the files.

const MAX_FILES = 80

export function buildAiCommitPrompt(
  files: ReadonlyArray<{ status: string; path: string }>,
  opts: { includeMerge: boolean; branchName: string },
): string {
  const fileLines = files.slice(0, MAX_FILES).map((f) => `${f.status}: ${f.path}`)
  if (files.length > MAX_FILES) {
    fileLines.push(`...and ${files.length - MAX_FILES} more files`)
  }
  let prompt = `Review the following changes in this repository and create a well-structured commit with an appropriate commit message.\n\nChanged files:\n${fileLines.join('\n')}`
  if (opts.includeMerge) {
    prompt += `\n\nAfter committing, merge the branch "${opts.branchName}" back into main and resolve any conflicts. Once merged, remove the worktree with "git worktree remove" and delete the branch with "git branch -d ${opts.branchName}".`
  }
  return prompt
}
