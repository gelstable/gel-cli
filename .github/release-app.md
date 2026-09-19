# The release GitHub App

The release workflows authenticate as a GitHub App installation token rather
than a personal access token. The App is owned by the `gelstable` organization
and is installed on every repository that runs this release pipeline.

Creating a GitHub App cannot be scripted: there is no REST endpoint for it, and
installing an App on an organization is deliberately a human consent step. This
file records the configuration so the App can be recreated, audited, or
replicated without reconstructing the reasoning.

## Why not the default token

`GITHUB_TOKEN` cannot be used. Pushes authenticated with it never fire workflow
events, so the required stable merge gate would never start on the generated
release PR head and the PR could never satisfy branch protection. See
`branch-protection.md` for the full mechanism. App installation tokens do fire
those events, which is the property the pipeline depends on.

## Why not `knope-bot`

The `knope-bot` App already installed on this organization cannot serve as the
release identity, for two independent reasons. It lacks `actions: write`, so it
cannot dispatch the stable merge gate. More fundamentally, its private key
belongs to Knope's maintainers: an installation token can only be minted by the
App's owner, so these workflows could never authenticate as it.

## Configuration

Owner: the `gelstable` organization. Create from
`https://github.com/organizations/gelstable/settings/apps/new`, not from
`https://github.com/settings/apps/new`. The latter creates a personally owned
App whose settings and private keys live under one individual's account, where
organization owners cannot manage it and continuity depends on that person
transferring ownership before they leave. Confirm the "Only on this account"
option names `@gelstable` rather than a username before creating.

| Field | Value |
| --- | --- |
| GitHub App name | `gel-releaser` (must be globally unique) |
| Homepage URL | `https://github.com/gelstable` |
| Webhook → Active | unchecked |
| Expire user authorization tokens | checked (the default) |
| Request user authorization (OAuth) during installation | unchecked |
| Enable Device Flow | unchecked |
| Where can this App be installed? | Only on this account |

The three user authorization settings govern OAuth user-to-server tokens, which
this App never issues: it acts only as itself, minting installation tokens.
Leave anything naming *user* authorization disabled. Token expiry stays enabled
because it is the safe default should that ever change.

### Repository permissions

| Permission | Level | Why |
| --- | --- | --- |
| Contents | Read and write | Push `knope/release-vN.x`, create tags and refs, upload release assets |
| Pull requests | Read and write | Open and refresh the generated release PR |
| Actions | Read and write | Dispatch the stable merge gate and the candidate workflow |
| Issues | Read-only | Read the PR timeline for label transitions |
| Metadata | Read-only | Mandatory; enabled automatically |

Leave every other permission at No access. Subscribe to no events: the pipeline
reacts to workflow events fired by the App's pushes, which is unrelated to
webhook delivery.

Adding a permission after creation requires an organization owner to approve the
change on the installation. The App will keep using the old permission set until
that approval happens.

### Verified behaviour

`GET /repos/{owner}/{repo}/collaborators/{user}/permission` backs the phase
authorization check that decides whether a maintainer had write access when they
applied a `prerelease:*` label. That endpoint was expected to need
`Administration: read` under App authentication. It does not: probed on
2026-09-19 against `gelstable/gel-cli`, it answered for both a token carrying the
installation's full set and a token down-scoped to `issues: read` plus
`metadata: read`. `Metadata: read` alone is therefore sufficient, and
`Administration` stays at No access.

`gel-cli` is public. Confirm this again if a release repository is ever made
private, since the endpoint's visibility rules differ there.

The PR timeline endpoint was probed in the same run and answered under the full
installation token.

## Installation

Install on `gelstable`, choosing **Only select repositories**, and select the
repositories running this pipeline: `gel-cli`, `gel`, and `gel-postgis`.

As installed on 2026-09-19:

```
app_slug: gelstable-releaser   app_id: 5001219   installation: 163010360
```

Installation and authentication are separate. A repository must be in the
installation *and* in the scope of `GEL_RELEASER_KEY` and
`GEL_RELEASER_APP_ID` before its workflows can mint a token. Both are currently
scoped to all three repositories. Changing that scope does not require the
private key:

```sh
gh api -X PUT /orgs/gelstable/actions/secrets/GEL_RELEASER_KEY/repositories \
  -F "selected_repository_ids[]=<id>"
```

Do not grant the App branch protection bypass. The pipeline is designed so the
release identity never needs it, and an exception here would defeat the merge
gate that authorizes publication.

## The private key

GitHub generates the private key once and retains only its public half. The PEM
is downloaded by the person who generates it and cannot be retrieved later.

Store it as an organization secret named `GEL_RELEASER_KEY`, scoped to the
selected repositories, and destroy the downloaded copy:

```sh
gh secret set GEL_RELEASER_KEY --org gelstable \
  --visibility selected --repos gel-cli < ~/Downloads/gel-releaser.*.pem
shred -u ~/Downloads/gel-releaser.*.pem
```

Record the numeric App ID as the organization variable `GEL_RELEASER_APP_ID`.
It is not a secret; workflows need it to mint installation tokens.

Anyone who can read `GEL_RELEASER_KEY` can mint a maximally scoped token for
every installed repository. Down-scoping tokens at mint time limits what each
job holds, but not what the key can do. Two things reduce that exposure: an App
may hold several private keys at once, so rotation can add a new key, cut over,
and delete the old one with no downtime; and the PEM can be held in an external
secrets manager and fetched through Actions OIDC, so that no long-lived secret
exists in the organization at all.

Generating an additional private key is a UI action. Scripted rotation is not
available.

## Using the App in workflows

Mint an installation token in every job that needs one. Do not mint once and
pass the token between jobs: installation tokens expire after one hour and
cannot be refreshed, several release jobs run longer than that, and passing a
token through job outputs widens where it can leak.

Down-scope each token to the repositories and permissions that job actually
needs rather than taking the installation's full set.

## Registry separation

`gel-registry` is not part of this App. It discovers published releases on a
schedule, verifies their attestations and bytes, and updates its own index using
its own `GITHUB_TOKEN`. The release repositories hold no registry credentials
and the registry holds no release credentials.

Keep it that way. One identity that can both produce an artifact and publish it
as the thing users install would collapse a separation this pipeline otherwise
maintains carefully. If the release repositories ever become private, give the
registry a separate read-only App for ingest rather than extending this one.
