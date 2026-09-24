# Releasing

This process is shared by Gel repositories. The **Build** step is the only
repository-specific part. A release manager needs write access to the repo.

1. **Prepare.** Open a normal PR titled `Release X.Y.Z` against `master` (or
   `release/N.x` for a backport). Update the version in the package manifest and
   lockfile, and add a `CHANGELOG.md` section for the version. Preview GitHub's
   generated release notes for the branch with `gh api -X POST
   repos/{owner}/{repo}/releases/generate-notes -f tag_name=vX.Y.Z -f
   target_commitish=<branch> --jq .body`. Edit that draft for users and copy it
   into the changelog before merging. Check that normal CI passes. Merge the PR.

2. **Build.** In Actions, select **Release**, choose the branch containing that
   merge, and click **Run workflow**. This repository reads `Cargo.toml`, builds
   all targets, packages archives and native packages, tests the built binaries
   and install managers, and creates a draft release. A run for an existing tag
   or published release fails before building.

3. **Review.** Open the draft for `vX.Y.Z`. Check its target commit, asset list,
   checksums, prerelease setting, and generated notes. Edit the notes to match
   the release PR's changelog section, then use the edited notes for publication.
   GitHub's generated notes use PR labels: `breaking` → Breaking changes,
   `feature` → New features, `fix` → Bug fixes, and everything else → Other
   changes. PRs labeled `internal` and Dependabot PRs are excluded. Keep labels
   current when merging PRs so the next draft reads well.

4. **Publish.** In the GitHub release editor, choose **Set as latest** as
   appropriate and click **Publish release**. GitHub creates the `vX.Y.Z` tag
   at the built commit then. The separate Gel registry app picks up published
   releases containing `gel-registry.json`; there is no registry step here.

5. **Handle variants and failures.** For a prerelease, use a version such as
   `X.Y.Z-rc.1` in the Release PR; the workflow marks its draft as a prerelease.
   For an older major, cut `release/N.x` from the last suitable commit, cherry-pick
   fixes into PRs against that branch, then prepare and run the release there.
   If a run fails, fix the cause on the branch and rerun. A rerun replaces an
   unpublished draft for the same tag; delete a stale draft manually if needed.
   Once a release is published, choose a new version for any further changes.
