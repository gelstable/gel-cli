# Release pipeline rehearsal — 2026-09-20

Repository: `gelstable/gel-cli-release-rehearsal`

Release line: `release/v7.x`

Generated release PR: [#2](https://github.com/gelstable/gel-cli-release-rehearsal/pull/2)

## Lifecycle results

| Step | Outcome | Evidence |
| --- | --- | --- |
| 1. Push a changeset | Passed. The protected v7 line advanced and Release PR prepared exactly `7.11.0`, not a prerelease. | [Release PR run 35462701556](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35462701556) |
| 2. Stage the stable candidate | Passed after hosted fixes. Draft release 392228905 exists, candidate run succeeded, the record was committed as generated head `8825bb2d43fda979f85679fd828c40ce67800550`, and the required gate passed on that exact head. | [Candidate run 35524356791](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35524356791), [head-attached gate 35526908526](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35526908526), [draft v7.11.0](https://github.com/gelstable/gel-cli-release-rehearsal/releases/tag/untagged-1f09b3b1efce8ffcc4e5) |
| 3. Add `prerelease:rc` | Passed after one hosted fix. The authorized preview path published `v7.11.0-rc.1` from derived commit `1e4dc3de22dd97212028757eacd3ccfbd7114ac3`; it is a prerelease and `latest` remained `v7.10.2`. A later controller fixed-point fix was a meaningful source update and correctly published `v7.11.0-rc.2` from `87c0ef80a07d0fc099f294c9028f42a248b3e732`. | [RC.1 candidate 35543130560](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35543130560), [RC.1 publish 35545051879](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35545051879), [v7.11.0-rc.1](https://github.com/gelstable/gel-cli-release-rehearsal/releases/tag/v7.11.0-rc.1), [RC.2 candidate 35546488118](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35546488118), [RC.2 publish 35548611920](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35548611920) |
| 4. Push a generated-only commit | Passed after one hosted fix. Commit `fadc9bc3d0929a65f1373843abc6beb8013eb1cf` changed only `packaging/release-candidate.json`; the meaningful snapshot stayed `23d779f4d1c5f3c7edeee0f1289bd195fb4c931fed503462cfe34cf0cc0e79ec`. The controller completed without dispatching a candidate, and no `v7.11.0-rc.3` draft, tag, or release appeared. | [Controller 35548806807](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35548806807) |
| 5. Remove the label | Passed. Removing `prerelease:rc` selected stable staging, rebuilt and verified the stable distribution, replaced the stale generated record, and advanced the PR to `d029a9c03cdc71952b222e0a38ff422660ab9e5f`. The required stable merge gate passed on that exact successor head. | [Stable candidate 35673827666](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35673827666), [head-attached gate 35677130967](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35677130967) |
| 6. Squash-merge the release PR | Passed. The human-authorized squash merge created release-line commit `f5407f4ace4dd635de39c0e46ea98c9735fd9f35`. Publication produced the non-prerelease `v7.11.0` release with 23 assets, the tag points exactly to the squash commit, and GitHub selected it as `latest`. | [Stable publication 35701547326](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35701547326), [v7.11.0](https://github.com/gelstable/gel-cli-release-rehearsal/releases/tag/v7.11.0) |
| 7. Re-run publication | Passed. Attempt 2 of the same publication run completed successfully. The repository still has exactly one `v7.11.0` release (ID 392228905), one exact tag ref, the same publication timestamp, and the same `latest` release. | [Publication rerun 35701547326](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35701547326) |
| 8. Open an ordinary backport PR | Passed. Disposable PR #10 changed docs only. Both controller events classified it as an ordinary backport and skipped all validation, derivation, ref creation, and candidate-dispatch steps. Both stable gates took the no-candidate-record safe path and passed. No release candidate workflow was dispatched; the PR and branch were closed and removed without merging. | [PR #10](https://github.com/gelstable/gel-cli-release-rehearsal/pull/10), [controller 35702551127](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35702551127), [controller 35702567691](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35702567691), [gate 35702551089](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35702551089), [gate 35702567772](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35702567772) |

## Step 2 hosted iterations

| Candidate run | Outcome |
| --- | --- |
| [35463019838](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35463019838) | Failed before staging: cargo-deb rejected the cross-target Debian path and macOS selected unsupported Python 3.14. |
| [35467010981](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35467010981) | Builds, smoke checks, and install scenarios passed; archive inspection failed when `grep -q` caused tar to receive SIGPIPE under `pipefail`. |
| [35470074473](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35470074473) | Staging created the draft; independent verification received HTTP 403 because its job token lacked draft visibility. |
| [35509394964](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35509394964) | Builds and install scenarios passed; staging rejected the prior draft after the release-line base advanced. |
| [35513101477](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35513101477) | Candidate, draft replacement, API verification, record commit, and cleanup passed. Gate [35516353565](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35516353565) attached to the successor head but received the same draft-visibility 403. |
| [35524356791](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35524356791) | Passed completely. Gate [35526908526](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35526908526) passed on record head `8825bb2d43fda979f85679fd828c40ce67800550`. |

## Fixes proven by the rehearsal

- Debian packaging uses cargo-deb's canonical target path, while RPM keeps its explicit target placeholder.
- Candidate Python is pinned to 3.13 on every runner.
- Archive listing checks consume their complete input under `pipefail`.
- Draft-verification jobs use the minimum `GITHUB_TOKEN` push-access grant GitHub requires to expose unpublished drafts; neither job mutates repository contents.
- A stale same-line/same-PR draft can be replaced only after GitHub proves its recorded base is an ancestor of the current line base.
- The candidate record commit triggers the required gate on the new generated PR head, and that exact head is the one verified.
- Direct-upgrade fixtures select the same registry channel as the compiled CLI, so prerelease candidates exercise the local testing-channel fixture instead of falling through to the network registry.
- An already-published preview snapshot can return an empty controller identity without skipped downstream steps attempting to parse `fromJSON('')`.

## Step 3 and Step 4 hosted iterations

| Run | Outcome |
| --- | --- |
| [RC candidate 35540260163](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35540260163) | All builds and smoke checks passed, but direct install scenarios failed on Linux, macOS, and Windows because the fixture advertised only the stable channel while the RC binary selected testing. |
| [RC candidate 35543130560](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35543130560) | Passed completely after matching the fixture channel to the candidate version; publication produced `v7.11.0-rc.1` without changing `latest`. |
| [Generated-only controller 35545637253](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35545637253) | Correctly resolved no candidate, then failed because GitHub evaluated downstream `fromJSON('')` environment expressions even though their steps were skipped. No preview candidate was dispatched. |
| [RC candidate 35546488118](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35546488118) | Passed completely for the meaningful controller-fix snapshot; publication produced `v7.11.0-rc.2`. |
| [Generated-only controller 35548806807](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35548806807) | Passed at the published-snapshot fixed point. No candidate workflow ran and no `v7.11.0-rc.3` was created. |
| [Stable candidate 35673827666](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35673827666) | Passed completely after the RC label was removed; committed the replacement stable record and produced a green required gate on successor head `d029a9c0`. |

The duplicate controller trigger observed during refresh retries was contained by cancelling the redundant run before it could dispatch a second candidate. It remains follow-up work outside the Step 2 proof.

## Stable publication and ordinary-backport evidence

- Stable publication attempt 1 created release ID 392228905 at `2026-09-22T07:51:49Z`; attempt 2 reused it rather than creating another release or tag.
- `refs/tags/v7.11.0` resolves directly to squash commit `f5407f4ace4dd635de39c0e46ea98c9735fd9f35`, and the latest-release endpoint returns the same release ID.
- The ordinary-backport gate logged `ordinary backport PR has no candidate record changes; stable gate is not applicable` before succeeding.
- The ordinary-backport controller's classifier returned the neutral path: every generated-release validation, snapshot, authorization, derivation, candidate-ref, and dispatch step was skipped.
- The initial backport PR open exercised the gate only because the controller intentionally excludes the `opened` event. A follow-up branch push was accepted before GitHub emitted its delayed `synchronize` event; closing and reopening the disposable PR also exercised the configured `reopened` event. Both resulting controller runs completed successfully and dispatched no candidate.

## Post-lifecycle release-description rehearsal

After the lifecycle rehearsal exposed machine identity JSON as the visible GitHub release description, a bounded follow-up moved that identity into a hidden HTML comment and selected the matching `CHANGELOG.md` section as the visible release notes. Rehearsal repair PR [#11](https://github.com/gelstable/gel-cli-release-rehearsal/pull/11) advanced the protected v7 line, and release PR [#12](https://github.com/gelstable/gel-cli-release-rehearsal/pull/12) prepared `7.12.0` from the accumulated pending changes.

Controller attempt 1 failed before mutation while the hosted runner recovered a partially installed Rust toolchain. Re-running the same workflow succeeded and dispatched [candidate run 35712720582](https://github.com/gelstable/gel-cli-release-rehearsal/actions/runs/35712720582), which passed all builds, smoke checks, install-manager scenarios, staging, independent API verification, candidate-record commit, and cleanup.

Draft release [v7.12.0](https://github.com/gelstable/gel-cli-release-rehearsal/releases/tag/untagged-46c4ac6f55e379da0798) (ID 393639751) remains unpublished for inspection. An independent API check proved that its visible body equals the exact `## 7.12.0 (2026-09-22)` changelog section, contains no visible candidate JSON, and contains exactly one parseable `gel-candidate-identity` HTML comment. The record advanced PR #12 to `dd1f7d7584f6b406cace4b0eef2c75a2ec36630a`; the required stable merge gate and the full hosted CI matrix passed on that head. PR #12 remains open and was not merged or published.
