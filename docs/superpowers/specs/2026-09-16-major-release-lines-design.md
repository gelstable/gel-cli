# Major release lines and preview releases

## Purpose

Gel CLI releases come from long-lived major-version branches. A release line
can stabilize and ship minor and patch versions while `master` continues to
move. A generated release pull request is the review and stable-publication
gate for each version. Maintainers can label that open PR to publish successive
alpha, beta, or release-candidate builds for users to try before stable.

This replaces the single `knope/release` PR against `master` as the release
lifecycle. The native artifact inventory, deterministic packaging, digest and
manifest generation, installation matrix, draft verification, and source-tree
equivalence checks developed on `feat/native-release-pipeline` are reference
mechanics for the new implementation. The implementation starts from `master`
in a separate worktree and ports those mechanics selectively.

## Branch and version ownership

- A maintainer cuts `release/v<major>.x` from `master` when that major enters
  its release cycle. The branch lives indefinitely and owns every minor and
  patch release of that major. It remains protected, including after a newer
  major ships. `master` does not publish Gel CLI releases.
- Fixes normally land on `master` first. Maintainers select the ones needed on
  a release line and cherry-pick them there, retaining the originating commit
  in the cherry-pick message where practical. Conflicts are resolved on the
  release line. A fix specific to an older line may live there alone.
- Each releasable change on a release line carries a change file. Change files
  on that branch determine its next minor or patch version; a major line never
  prepares a version with a different major. The line's own version and
  changelog state determine subsequent versions independently of `master` and
  other release lines. Cutting the next major line includes setting its
  starting version deliberately; no release number is inferred from the
  current `Cargo.toml` on `master` alone.
- A push to `release/v<major>.x` with pending change files opens or refreshes
  one generated PR from `knope/release-v<major>.x` into that line. The PR
  contains the prepared stable version in `Cargo.toml` and `Cargo.lock`, the
  changelog update, and later its stable candidate record. A line with no
  pending change files has no new release PR. Each merged release closes that
  version's PR; the next change starts a new one.
- Multiple major lines can have release PRs and candidates simultaneously.
  Automation is keyed by major line and validates that a PR's head and base
  names match that line. A PR from a fork or an unrelated branch cannot
  publish.

For example, `release/v7.x` may publish `7.0.0`, `7.1.0`, then `7.1.1`
while work on `master` and `release/v8.x` continues independently.

## PR phase labels and automatic preview publication

The only release phase labels are `prerelease:alpha`, `prerelease:beta`, and
`prerelease:rc`. Exactly one may be active on a generated release PR. A
write-authorized maintainer's label transition authorizes automatic
publication to `testing`; a PR with conflicting phase labels cannot publish.
The controller responds to label additions and removals, PR updates, and an
explicit retry dispatch. It reads the PR's current state rather than trusting
the event payload's possibly stale label set. The release-preparation workflow
dispatches the same controller after refreshing a bot-owned PR, so publication
does not depend on a bot-generated PR event firing another workflow.

With an active phase label, the controller publishes a preview for the current
prepared stable version and source snapshot. The first alpha build is
`X.Y.Z-alpha.1`; subsequent meaningful source updates produce `.2`, `.3`,
and so on. Switching to beta or rc starts that phase at `.1`, even if source
bytes have not changed. A given phase and source snapshot is published at most
once. Alpha, beta, and rc progression is selected by maintainers, never
inferred from elapsed time or test results. `-dev.N` and `nightly` are outside
this pipeline. If new change files alter the prepared base version while a
phase label remains, that new base version begins its own phase sequence at
`.1`; already published versions remain unchanged.

A source snapshot is the generated PR's code and prepared release metadata,
excluding only the workflow's own candidate-record and generated distribution
files. A commit that changes only those generated files cannot trigger another
preview. The workflow freezes a PR head SHA, derives a temporary versioned
commit from that tree by changing `Cargo.toml` and `Cargo.lock` to the selected
prerelease version, and builds that exact commit. The PR itself retains its
planned stable version. The preview Git tag points to the versioned commit;
its candidate record also names the original PR head. The derived commit stays
reachable until the tag is created.

Runs for a release line are serialized. The next suffix is chosen from
published tags for the same base version and phase. A failed or stale draft
reuses the next unpublished suffix on retry rather than leaving a gap in
published numbers. Immediately before publication, the workflow confirms that
the PR is still open, the same phase is active, and its meaningful source
snapshot is still current. If any condition changed, it leaves the draft
unpublished and lets the newest event rebuild or refresh the candidate.
Retries of a published snapshot do nothing; published assets and tags are
immutable.

Removing the last phase label selects stable staging. A label swap may cause
both `labeled` and `unlabeled` events; the controller resolves them from the
current labels and rechecks that state before recording or publishing a
candidate. An intermediate no-label event must not allow a stale stable
candidate through the merge gate.

## Stable candidate and publication

Without a phase label, the controller stages a draft `vX.Y.Z` release from
the generated PR's prepared stable version. The draft is verified and its
candidate record is committed to that PR. Required checks prove that the
record matches the draft assets, that the PR is based on the current release
line, that its version is a plain `X.Y.Z` in the line's major, and that the
prospective merge tree is source-equivalent to the tested candidate outside
the explicitly generated metadata paths. A phase-labeled PR cannot pass the
stable merge gate.

Merging the PR into `release/v<major>.x` authorizes stable publication. The
publication workflow runs on that release-line push, verifies the record
against the actual merge commit and draft assets, creates `vX.Y.Z` at the
merge commit, and publishes the existing draft without rebuilding or replacing
assets. Re-running it is safe: an existing matching tag and published release
are accepted; a tag at another commit or different asset bytes stops the run.
Ordinary backport pushes to a release line do not publish anything without a
new, merge-matching candidate record.

Published previews are GitHub prereleases and are never marked latest. A
stable release is marked GitHub latest only if it is the highest published
stable SemVer across all release lines. A later patch on v7 must not replace a
newer v8 release as latest. A repository-wide publication lock and a fresh
comparison immediately before the API update prevent concurrent lines from
racing on this setting.

## Artifacts and registry channels

Every candidate uses the same native target inventory, deterministic archives,
Linux packages, digest manifests, build provenance, and install checks. The
install matrix runs on the actual candidate executables and generated `.deb`
and `.rpm` before a draft is staged. Candidate verification reads uploaded
assets back from GitHub. A preview's candidate record is durably associated
with its draft as a `gel-candidate.json` release asset outside the stable
release PR. The record lists the identities and digests of distribution assets
but does not recursively list itself. Retries read it to verify the same
identity. The stable candidate record remains in its PR. Publication never
rebuilds reviewed bytes.

`gel-registry.json` explicitly places plain `X.Y.Z` packages in `stable` and
`X.Y.Z-alpha.N`, `-beta.N`, or `-rc.N` packages in `testing`. Unsupported
suffixes fail before the build matrix. Prerelease package-manager versions
sort below their matching stable versions. The GitHub release's prerelease
flag agrees with the manifest channel, but the registry reads the manifest's
explicit channel rather than inferring it from that flag. The registry only
discovers published, non-draft releases from its allowlist and promotes them
through its own reviewable snapshot PR. Publishing a GitHub release does not
immediately change the public registry root.

The CLI's legacy default package root is a separate configuration decision;
this design does not silently change it. No `edgedb` alias or nightly release
is added. Downstream package-manager automation may be notified of all
published versions, but it must not make an older major the default when a
newer stable major exists.

## Workflow boundaries and recovery

The release controller selects a line, PR, phase, source snapshot, and version.
The build/stage workflow accepts that immutable identity and produces assets
plus a draft candidate record. Verification checks both the bytes and their
source identity. Separate preview and stable publication steps apply their
different authorization gates. Shared build, package, install, and verify
mechanics do not embed assumptions about `master` or a single release line.

Each workflow checks its own inputs at the point of side effect. A moved PR,
changed phase, changed base branch, missing asset, failed installation test,
or mismatched tag fails closed before publication. A failed draft can be
restaged for its unpublished version; a published tag is never moved. A failed
stable publication can be retried against the same merge commit. Operators can
see the selected line, PR, phase, source SHA, version, draft release ID, and
reason for rejection in workflow output and the candidate record.

## Verification and migration

Automated tests cover branch/PR identity, version selection, phase changes,
suffix allocation and retry, stale-source rejection, the stable merge gate,
GitHub latest selection across majors, registry manifest channels, package
versions, and idempotent publication. Workflow validation checks triggers,
permissions, action pins, and that no code is rebuilt at publication. The
native install matrix remains a candidate-stage gate.

Implementation begins in a clean worktree from `master`. Port the existing
artifact, digest, package, manifest, and verification modules and their tests
where they fit; redesign the four branch-specific workflows and release state
around this lifecycle. Preserve `feat/native-release-pipeline` and its current
uncommitted work as a reference until the new pipeline has passed its checks.
