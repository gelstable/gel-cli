"""Shared setup for the release packaging tests."""

import os

# gel_release reads the operating repository from the environment at import
# time, and this suite's fixtures assert against the production repository.
# Pin it before any test module imports gel_release so the suite stays
# deterministic even where GITHUB_REPOSITORY is already set, such as a CI run
# in a fork.
os.environ["GITHUB_REPOSITORY"] = "gelstable/gel-cli"
