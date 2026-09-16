"""Strict models for persisted and external release data."""

from __future__ import annotations

from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field, HttpUrl, model_validator

PositiveInt = Annotated[int, Field(strict=True, gt=0)]
Sha256 = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Blake2b512 = Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]
GitSha = Annotated[str, Field(pattern=r"^[0-9a-f]{40}$")]


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
    schema_version: Literal[1]
    version: Annotated[str, Field(pattern=r"^[0-9]+\.[0-9]+\.[0-9]+(?:[-+].+)?$")]
    tag: Annotated[str, Field(min_length=2)]
    draft_release_id: PositiveInt
    source_sha: GitSha
    build_date: Annotated[str, Field(min_length=1)]
    workflow_runs: list[WorkflowRun]
    attestation: AttestationRecord
    assets: list[CandidateAsset]

    @model_validator(mode="after")
    def validate_identity(self) -> CandidateRecord:
        expected = f"v{self.version}"
        if self.tag != expected:
            raise ValueError(f"tag must be {expected}")
        names = [asset.name for asset in self.assets]
        if len(names) != len(set(names)):
            raise ValueError("candidate asset names must be unique")
        if self.attestation.subject_count != len(self.assets):
            raise ValueError("attestation subject_count must match asset count")
        return self


class GithubAsset(StrictModel):
    id: PositiveInt
    name: Annotated[str, Field(min_length=1)]
    size: PositiveInt


class ManifestVerification(StrictModel):
    size: PositiveInt
    blake2b: Blake2b512
    sha256: Sha256


class ManifestInstallRef(StrictModel):
    ref: HttpUrl
    type: Annotated[str, Field(min_length=1)]
    encoding: Literal["identity", "zstd"]
    verification: ManifestVerification


class VersionDetails(StrictModel):
    major: Annotated[int, Field(strict=True, ge=0)]
    minor: Annotated[int, Field(strict=True, ge=0)]
    patch: Annotated[int, Field(strict=True, ge=0)]
    prerelease: list[str]
    metadata: list[str]


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
    channel: Literal["stable"]
    platform: str
    packages: list[ManifestPackage]


class ReleaseManifest(StrictModel):
    schema_version: Literal[1]
    indexes: list[ManifestIndex]
