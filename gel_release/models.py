"""Strict models for persisted and external release data."""

from __future__ import annotations

import re
from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, HttpUrl, model_validator

PositiveInt = Annotated[int, Field(strict=True, gt=0)]
Sha256 = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Blake2b512 = Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]
GitSha = Annotated[str, Field(pattern=r"^[0-9a-f]{40}$")]
SourceSnapshot = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]

_BASE_VERSION_COMPONENT = r"(?:0|[1-9][0-9]*)"
_STABLE_VERSION_PATTERN = re.compile(
    rf"^{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT}$"
)
_PREVIEW_VERSION_PATTERN = re.compile(
    rf"^(?P<base>{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT})"
    r"-(?P<phase>alpha|beta|rc)\.(?P<number>[1-9][0-9]*)$"
)
_LINE_PATTERN = re.compile(r"^release/v(?P<major>[1-9][0-9]*)\.x$")


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class CandidateAsset(StrictModel):
    id: PositiveInt
    name: Annotated[str, Field(min_length=1)]
    size: PositiveInt
    sha256: Sha256
    blake2b512: Blake2b512


class WorkflowRun(StrictModel):
    workflow: Annotated[str, Field(min_length=1)]
    run_id: PositiveInt
    run_attempt: PositiveInt


class AttestationRecord(StrictModel):
    predicate_type: Annotated[str, Field(min_length=1)]
    subject_count: PositiveInt


class CandidateRecord(StrictModel):
    schema_version: Literal[2]
    line: Annotated[str, Field(pattern=r"^release/v[1-9][0-9]*\.x$")]
    pr_number: PositiveInt
    phase: Literal["alpha", "beta", "rc"] | None
    version: Annotated[
        str,
        Field(
            pattern=(
                rf"^{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT}\.{_BASE_VERSION_COMPONENT}"
                r"(?:-(?:alpha|beta|rc)\.[1-9][0-9]*)?$"
            )
        ),
    ]
    tag: Annotated[str, Field(min_length=2)]
    draft_release_id: PositiveInt
    source_sha: GitSha
    source_snapshot: SourceSnapshot
    build_sha: GitSha
    base_sha: GitSha
    build_date: Annotated[str, Field(min_length=1)]
    workflow_runs: list[WorkflowRun]
    attestation: AttestationRecord
    assets: list[CandidateAsset]

    @model_validator(mode="after")
    def validate_identity(self) -> CandidateRecord:
        line_match = _LINE_PATTERN.fullmatch(self.line)
        if line_match is None:
            # Keep the explicit message useful when this is called directly
            # instead of relying on Pydantic's field error.
            raise ValueError(f"line must be a release/v<major>.x branch: {self.line!r}")

        stable_match = _STABLE_VERSION_PATTERN.fullmatch(self.version)
        preview_match = _PREVIEW_VERSION_PATTERN.fullmatch(self.version)
        if stable_match is None and preview_match is None:
            raise ValueError(f"unsupported candidate version {self.version!r}")

        version_major = int(self.version.split(".", 1)[0])
        line_major = int(line_match.group("major"))
        if line_major != version_major:
            raise ValueError(
                f"line {self.line} major {line_major} does not match version major {version_major}"
            )

        expected = f"v{self.version}"
        if self.tag != expected:
            raise ValueError(f"tag must be {expected}")

        if self.phase is None:
            if stable_match is None:
                raise ValueError("phase must match the prerelease version")
            if self.build_sha != self.source_sha:
                raise ValueError("stable candidate build_sha must equal source_sha")
        else:
            if preview_match is None or preview_match.group("phase") != self.phase:
                raise ValueError("phase must match the prerelease version")
            if self.build_sha == self.source_sha:
                raise ValueError("preview candidate build_sha must differ from source_sha")

        names = [asset.name for asset in self.assets]
        if len(names) != len(set(names)):
            raise ValueError("candidate asset names must be unique")
        ids = [asset.id for asset in self.assets]
        if len(ids) != len(set(ids)):
            raise ValueError("candidate asset ids must be unique")
        if "gel-candidate.json" in names or "packaging/release-candidate.json" in names:
            raise ValueError("candidate record must not include itself in assets")
        if self.attestation.subject_count != len(self.assets):
            raise ValueError("attestation subject_count must match asset count")
        return self


class GithubAsset(StrictModel):
    id: PositiveInt
    name: Annotated[str, Field(min_length=1)]
    size: PositiveInt


class ManifestVerification(StrictModel):
    size: Annotated[int, Field(strict=True, ge=0)]
    blake2b: Blake2b512
    sha256: Sha256 | None = None


class ManifestInstallRef(StrictModel):
    ref: HttpUrl
    type: Annotated[str, Field(min_length=1)]
    encoding: Annotated[str, Field(min_length=1)] | None = None
    verification: ManifestVerification


class PrereleaseDetail(StrictModel):
    phase: Literal["alpha", "beta", "rc"]
    number: Annotated[int, Field(strict=True, ge=0)]


class VersionDetails(StrictModel):
    major: Annotated[int, Field(strict=True, ge=0)]
    minor: Annotated[int, Field(strict=True, ge=0)]
    patch: Annotated[int, Field(strict=True, ge=0)]
    prerelease: list[PrereleaseDetail]
    metadata: dict[str, str]


class ManifestPackage(StrictModel):
    basename: Literal["gel-cli"]
    name: Literal["gel-cli"]
    version: str
    version_details: VersionDetails
    version_key: str
    revision: Literal["1"]
    build_date: str
    architecture: str
    slot: Literal[""]
    installref: HttpUrl
    installrefs: list[ManifestInstallRef]
    tags: dict[str, str]


class ManifestIndex(StrictModel):
    channel: Literal["stable", "testing"]
    platform: str
    packages: list[ManifestPackage]


class Replacement(StrictModel):
    """A legacy published artifact digest and its replacement release URL."""

    sha256: Annotated[str, Field(min_length=1)]
    url: Annotated[str, Field(min_length=1)]


class ReleaseManifest(StrictModel):
    # Published v7.10.x manifests carry ``replacements`` instead of ``indexes``;
    # the vendored JSON schema blesses both shapes, so this model must too.
    schema_version: Literal[1]
    indexes: list[ManifestIndex] = []
    replacements: list[Replacement] = []

    @model_validator(mode="after")
    def validate_shape(self) -> ReleaseManifest:
        if not self.indexes and not self.replacements:
            raise ValueError("a release manifest must contain indexes or replacements")
        return self
