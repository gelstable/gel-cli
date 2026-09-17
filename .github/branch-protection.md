# Branch protection for release lines

Configure branch protection (or an equivalent repository ruleset) before
creating each long-lived release line. Apply the rules to `master` and to
every active branch matching `release/v*.x`. A rule for the pattern must cover
all current and future release lines so that v7 and v8 cannot accidentally
use different merge controls.

## Required check

Every generated release PR targeting `release/v*.x` must require the status
check shown by GitHub as:

`Release candidate check / stable merge gate`

The job's check name is `stable merge gate`; select the complete
`Release candidate check / stable merge gate` context when GitHub presents
workflow and job names separately. Require the check to pass on the current
PR head and current prospective merge tree before allowing merge. Require the
branch to be up to date with its base before merging, and require a pull
request with the repository's normal review and conversation rules.

The protected branch rule still requires this status context for every PR;
ordinary backports receive their passing result from the safe path below.

Any combination of merge methods may be allowed on release lines. The
publication gate verifies merge commits, squashes, and rebases by binding the
recorded base through the pushed merge's first-parent chain, so no merge
method needs to be disabled. Keep "require branches to be up to date"
enabled: a squash or rebase that lands on a moved base is rejected at
publication.

An ordinary backport PR targeting a release line gets the safe passing path
when it cannot add or modify `packaging/release-candidate.json`. Preview
candidate records are release assets, never files in the repository, so they
never appear in a PR diff. The workflow identifies the generated release
PR by its shared line-to-head helper and runs the full stable merge gate for
that PR. A candidate record change on an ordinary backport fails the check.

Do not allow bypassing the required status check, including for repository
administrators or release automation. Disable force pushes and branch
deletion for `master` and active release lines. The release bot may update its
generated `knope/release-vN.x` branch through the workflow's narrowly scoped
token; that generated branch is not a protected release line and is never a
substitute for the required check on `release/vN.x`.

## How the gate reaches the generated head

Candidate staging pushes the reviewed record onto the generated
`knope/release-vN.x` head, and the branch rule evaluates the required context
on that commit. Two things make the check appear there.

First, the push itself must fire a `pull_request` `synchronize` event.
`secrets.RELEASE_BOT_TOKEN` must therefore be a personal access token or a
GitHub App installation token with write access to contents, pull requests,
and actions. The default `GITHUB_TOKEN` cannot be used: pushes authenticated
with it never trigger workflow events, so the required check would never start
on the new head and the release PR could never satisfy branch protection. The
workflows fail on their first step when the secret is missing rather than
falling back to `GITHUB_TOKEN`.

Second, the controller and the candidate commit job dispatch
`release-candidate-check.yml` on the generated head branch, never on a fixed
branch such as `master`. A `workflow_dispatch` check run attaches to the head
SHA of the ref it was dispatched on; a run dispatched on `master` posts its
result on `master` and can never satisfy a required context on the release PR
head. The dispatched run resolves every boundary through the live PR and
checks out the live head SHA. If the head branch moved between the dispatch
and the run, the run refuses to pass: it compares `GITHUB_SHA` with the live
head and fails, because otherwise it would record a verdict on a commit it did
not verify. The push that moved the branch fires its own `synchronize` event,
so the newer commit still gets its own run.

Dispatching on the generated head branch means GitHub runs the copy of
`release-candidate-check.yml` that exists on that branch. The generated branch
is cut from the release line, so a line must carry these workflows before its
first release PR, and a workflow change reaches the gate only once it has been
merged into the line. This is the same trust boundary as the candidate ref
below: review generated workflow changes on the release PR.

## Workflow trust boundary

The controller dispatches a candidate ref whose workflow YAML and release
tooling can run with write privileges during staging. Treat that candidate ref
as a trust boundary: protect generated workflow/tooling changes and review the
generated release PR before allowing it to merge. The `candidate ref` is
temporary and is removed after a successful candidate run. Failed candidate
runs retain the ref for recovery. A derived preview ref is removed only after
successful publication.

## Re-evaluation events

`Release candidate check` must run for every release PR `opened`,
`synchronize`, `reopened`, `labeled`, and `unlabeled` event. A maintainer label
transition therefore invalidates the old result immediately. A PR carrying
`prerelease:alpha`, `prerelease:beta`, or `prerelease:rc` cannot satisfy the
stable merge gate; remove the label and wait for the new check before merging.

## Line setup checklist

For a new line, create `release/vN.x` from `master`, protect it with the same
required check, and verify the check appears in a test pull request before
accepting release changes. Keep `master` protected with its regular checks as
well. The line-specific workflow owns the generated release PR and candidate
record, while the branch rule remains the final merge boundary.

Review the ruleset after changing the release workflow name or the gate job
name. If GitHub shows a new check context, update the required check before
merging any release PR and keep the old context required until the replacement
has passed end to end.
