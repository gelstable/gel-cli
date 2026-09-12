---
gel-cli: patch
---

Recognise package-manager-owned installs. `gel cli upgrade` and the background version check now print the owning manager's native upgrade command (Homebrew, Scoop, WinGet, Nix, apt, dnf, pacman) instead of replacing a file the manager owns. Fixes a case where a Scoop install, living under the user's home directory, could be overwritten and its shims broken.
