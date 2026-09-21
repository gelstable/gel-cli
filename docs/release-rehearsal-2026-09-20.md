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
| 5. Remove the label | Pending human authorization. | — |
| 6. Squash-merge the release PR | Pending human authorization. | — |
| 7. Re-run publication | Pending Step 6. | — |
| 8. Open an ordinary backport PR | Pending Step 7. | — |

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

The duplicate controller trigger observed during refresh retries was contained by cancelling the redundant run before it could dispatch a second candidate. It remains follow-up work outside the Step 2 proof.
