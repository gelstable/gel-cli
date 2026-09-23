"""Strict models for persisted and external release data."""

from __future__ import annotations

from typing import Annotated, Any, Literal

from pydantic import BaseModel, ConfigDict, Field, HttpUrl, model_validator

PositiveInt = Annotated[int, Field(strict=True, gt=0)]
Sha256 = Annotated[str, Field(pattern=r"^[0-9a-f]{64}$")]
Blake2b512 = Annotated[str, Field(pattern=r"^[0-9a-f]{128}$")]


class StrictModel(BaseModel):
    model_config = ConfigDict(extra="forbid", strict=True)


class ManifestVerification(StrictModel):
    """Byte verification for an install ref this pipeline generates.

    Deliberately stricter than ``Verification`` in the vendored JSON schema,
    which leaves ``sha256`` nullable and permits ``size: 0`` so that the
    registry can still read historical documents. Nothing this pipeline emits
    may omit a digest or describe a zero-byte artifact, so the model refuses
    what the schema tolerates. ``validate_manifest`` runs both checks; the
    model is the binding one for generated manifests.
    """

    size: PositiveInt
    blake2b: Blake2b512
    sha256: Sha256


class ManifestInstallRef(StrictModel):
    ref: HttpUrl
    type: Annotated[str, Field(min_length=1)]
    # ``build_manifest`` emits exactly these two encodings, one bare executable
    # and one zstd-compressed. The vendored schema allows any string; this
    # pipeline does not, so an unknown encoding is a generator bug, not input.
    encoding: Literal["identity", "zstd"]
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
    """The ``indexes`` manifest this pipeline generates.

    Strict by construction: every field is something ``build_manifest`` chose,
    so anything unexpected here is a defect in the generator rather than
    third-party data to be tolerated. ``replacements`` is not a member of this
    document at all -- ``extra="forbid"`` rejects it -- because the new
    pipeline never emits one.
    """

    schema_version: Literal[1]
    indexes: Annotated[list[ManifestIndex], Field(min_length=1)]


class LegacyReplacementsManifest(StrictModel):
    """A published v7.10.x manifest, kept readable exactly as shipped.

    This is historical data: the released assets are immutable and no future
    run of this pipeline will ever produce another one. Validation is
    therefore deliberately permissive -- digests and URLs are only required to
    be non-empty strings -- and its job is to let the registry promoter read
    the document, not to hold it to the current pipeline's standards.

    Published documents carry an empty ``indexes`` array alongside
    ``replacements``; a populated one would mean the document is really the
    new shape and must be validated as ``ReleaseManifest`` instead.
    """

    schema_version: Literal[1]
    replacements: Annotated[list[Replacement], Field(min_length=1)]
    indexes: list[Any] = []

    @model_validator(mode="after")
    def reject_populated_indexes(self) -> LegacyReplacementsManifest:
        if self.indexes:
            raise ValueError("a legacy replacements manifest must not carry indexes")
        return self
