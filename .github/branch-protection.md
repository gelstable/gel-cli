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

An ordinary backport PR targeting a release line gets the safe passing path
when it cannot add or modify `packaging/release-candidate.json` or
`packaging/gel-candidate.json`. The workflow identifies the generated release
PR by its shared line-to-head helper and runs the full stable merge gate for
that PR. A candidate record change on an ordinary backport fails the check.

Do not allow bypassing the required status check, including for repository
administrators or release automation. Disable force pushes and branch
deletion for `master` and active release lines. The release bot may update its
generated `knope/release-vN.x` branch through the workflow's narrowly scoped
token; that generated branch is not a protected release line and is never a
substitute for the required check on `release/vN.x`.

## Workflow trust boundary

The controller dispatches a candidate ref whose workflow YAML and release
tooling can run with write privileges during staging. Treat that candidate ref
as a trust boundary: protect generated workflow/tooling changes and review the
generated release PR before allowing it to merge. The `candidate ref` is
temporary and is removed after each candidate run, including failed runs.

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
