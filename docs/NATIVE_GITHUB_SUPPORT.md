# Native GitHub support

Kai's GitHub controls use the selected chat's execution checkout, including linked worktrees and nested project folders. Open the hub with **Control-Shift-G**, `/github`, Settings → GitHub & source control, or the sidebar GitHub widget. The widget can be disabled in Appearance.

## Everyday flows

- **Repository:** discover the checkout, push remote, branch, upstream, ahead/behind counts, and actual GitHub repository. Initialize a local repository, publish it privately or publicly, fetch, or pull with fast-forward only. Publishing creates a remote; committing/pushing remains a separate deliberate action.
- **Branches:** switch to an existing local/tracking branch or create a new branch. Existing worktree restrictions and Git conflict checks remain enforced.
- **Pull requests:** browse the project's open PRs, actual checks/review/draft status, or your open PRs across repositories. Open in the browser, review the native diff, check out a PR in its matching project, create a draft/ready PR, or confirm a squash merge. Merge requests match the reviewed head commit and honor GitHub protections. Forks resolve their parent repository and use an owner-qualified head.
- **Account:** view the authenticated account's yearly contribution calendar and recent accessible repositories; clone one into a new local folder and add it to Kai's projects.
- **History:** inspect recent commits in the native diff viewer.
- **Commit & push:** click the commit icon beside Copy on an assistant response, or use `/commit`. Review changed paths, binary markers, line totals, branch, push destination, and outgoing commit count. Enter a message and explicitly choose Commit & push or Commit only. If only local commits remain, Push sends them without creating an empty commit.

The response icon opens the **current workspace** review, including staged, unstaged, and untracked files (excluding ignored files). It does not pretend to commit only the response's edits. The separate **Edited files** card is an immutable per-prompt checkpoint comparison: totals, expandable paths, Undo, and View changes. Clicking a path opens its saved diff. Appearance → Response changes → Compact diff mode replaces the card with clickable green/red totals beside Copy and Auto Push; those totals open the same saved diff viewer. Earlier checkpoints are counted lazily when visible; imported chats without saved before/after files cannot acquire a fabricated historical diff.

The native diff viewer has an expandable folder tree, per-file addition/deletion counts, binary annotations, full selected-file totals, and paged patch content. GitHub PR and commit reviews use the same viewer, with their source clearly identified.

## Credentials and mutations

Kai discovers the installed `gh` CLI and its existing account. GitHub tokens stay inside `gh`; Kai does not copy them into renderer state, settings, or logs. Existing Git SSH/HTTPS credentials remain usable. The sign-in button opens `gh auth login --web` in Terminal. Custom GitHub Enterprise hosts require an existing `gh` host registration before API access; inherited enterprise token variables cannot silently authenticate an arbitrary repository hostname.

Mutations use captured checkout identities and a canonical common-directory lease shared across linked worktrees. Local mutations are blocked while Kai is running an agent in that checkout. A commit review fingerprints HEAD, branch, remote, index, file bytes, symlink targets, and file modes. Changed reviews must be refreshed. Staging uses an isolated index under the real index lock; normal Git hooks/signing run. The committed index is published atomically. Failed pushes keep the local commit and support retry. A post-commit failure that already saved HEAD reconciles the index, reports the saved commit, and waits for an explicit push. No force push, hard reset, automatic merge, or automatic publication occurs.

Read probes disable external diff/textconv and configured clean filters. Explicit commits retain normal Git filters/hooks. Processes have deadlines, bounded output, cancellation, and owned-process cleanup.

## Resource bounds

Reads run on the existing background executor. Repository snapshots are cached for up to 16 roots and refreshed on demand, with a 60-second active-surface interval and coalesced completion refreshes; there is no permanent polling timer or second renderer. Sidebar growth and overlay transitions have finite animation lifetimes and respect reduced motion. Branches, sync files, and diff trees are virtualized. Repository PRs page in batches of 30; personal PRs and history are bounded at 100, the branch catalog at 512, and detailed workspace review at 4,096 paths / 512 MB. Response summaries keep at most 128 cached entries with 24 display paths each; full counts persist in checkpoint SQLite manifests.

## Reference and validation

Reviewed [T3 Code source control](https://github.com/pingdotgg/t3code/blob/6d46336476941257d774be7c7733e9caf201fec2/docs/user/source-control.md), its checkout mutation leases, source-control discovery, GitHub credentials/PR provider, and Git action workflow at that revision. Kai retains its Rust/GPUI/AppKit stack and existing checkpoint/agent infrastructure; this is not a claim of every T3 feature or GitHub website feature being reproduced.

Automated coverage includes real temporary repositories and bare remotes, staged/unstaged/untracked/ignored paths, stale reviews and file modes, linked worktrees, first commits, failed hooks and rejected pushes, post-commit interruption recovery, fork PR argument routing, unrelated-project checkout rejection, process cancellation/output/deadline limits, rendered click-only widget/panel behavior, response Copy/Commit alignment, immutable change counts, folded-response cards, and native diff tree disclosure. Explicit live checks only read the existing `gh` account, repository metadata, open PRs, contribution calendar, and repositories; they do not create/push/merge anything on GitHub.

Full release performance qualification and human visual review remain separate from these correctness checks.
