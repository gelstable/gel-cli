# Changelog

Preview and edit GitHub's generated notes in each Release PR, then copy them
into a new version section here before merging. Match the final draft release
notes to that section before publishing.

## Unreleased

- Added configurable registry sources for portable package catalogs, with
  ordered mirror fallback and conflict diagnostics.
- `gel cli upgrade` and the background version check now direct users of
  package-manager-owned installs to the owning manager's upgrade command
  (Homebrew, Scoop, WinGet, Nix, apt, dnf, or pacman) instead of replacing its
  files. Scoop detection also handles relocated or renamed roots.
