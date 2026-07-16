//! Package index parsing and validation.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;
use url::Url;

use super::source::Source;
use super::types::{
    ArtifactIdentity, Blake2bDigest, CliPackage, Compression, ExtensionPackage, PackageType,
    ServerPackage,
};
use crate::portable::ver;

#[derive(Deserialize, Debug, Clone)]
pub struct IndexDocument {
    pub packages: Vec<PackageData>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct InstallRef {
    #[serde(rename = "ref")]
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub encoding: Option<String>,
    pub verification: Verification,
}

#[derive(Deserialize, Debug, Clone)]
pub struct PackageData {
    pub basename: String,
    pub version: String,
    pub installrefs: Vec<InstallRef>,
    pub slot: String,
    #[serde(default)]
    pub tags: HashMap<String, String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct Verification {
    pub size: u64,
    pub blake2b: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ValidatedIndex {
    artifacts: Vec<ValidatedArtifact>,
}

impl ValidatedIndex {
    pub(crate) fn into_artifacts(self) -> Vec<ValidatedArtifact> {
        self.artifacts
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ValidatedArtifact {
    Server(ArtifactIdentity, ServerPackage),
    Cli(ArtifactIdentity, CliPackage),
    Extension(ArtifactIdentity, ExtensionPackage),
}

impl ValidatedArtifact {
    pub(crate) fn identity(&self) -> &ArtifactIdentity {
        match self {
            Self::Server(identity, _) | Self::Cli(identity, _) | Self::Extension(identity, _) => {
                identity
            }
        }
    }

    pub(crate) fn url(&self) -> &Url {
        match self {
            Self::Server(_, package) => &package.url,
            Self::Cli(_, package) => &package.url,
            Self::Extension(_, package) => &package.url,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("invalid package {package:?} in registry index {index_source}: field {field}: {message}")]
pub(crate) struct IndexValidationError {
    index_source: String,
    package: String,
    field: &'static str,
    message: String,
}

impl IndexValidationError {
    fn new(
        index_source: &Source,
        package: &PackageData,
        field: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self {
            index_source: index_source.display(),
            package: package.basename.clone(),
            field,
            message: message.into(),
        }
    }
}

impl IndexDocument {
    pub fn from_slice(bytes: &[u8]) -> anyhow::Result<IndexDocument> {
        let jd = &mut serde_json::Deserializer::from_slice(bytes);
        Ok(serde_path_to_error::deserialize(jd)?)
    }
    pub(crate) fn validate(
        &self,
        index_source: &Source,
        artifact_base: &Source,
        platform: &str,
    ) -> Result<ValidatedIndex, IndexValidationError> {
        let mut artifacts = Vec::new();
        for package in &self.packages {
            if let Some(artifact) =
                validate_package(index_source, artifact_base, platform, package)?
            {
                artifacts.push(artifact);
            }
        }
        Ok(ValidatedIndex { artifacts })
    }
}

fn cli_media_type(platform: &str) -> Option<&'static str> {
    if platform.ends_with("-apple-darwin") {
        Some("application/x-mach-binary")
    } else if platform.contains("-linux-") {
        Some("application/x-pie-executable")
    } else if platform.contains("-windows-") {
        Some("application/x-dosexec")
    } else {
        None
    }
}

fn validate_package(
    index_source: &Source,
    artifact_base: &Source,
    platform: &str,
    package: &PackageData,
) -> Result<Option<ValidatedArtifact>, IndexValidationError> {
    if matches!(package.basename.as_str(), "gel-server" | "edgedb-server") {
        return validate_server(index_source, artifact_base, package).map(Some);
    }
    if matches!(package.basename.as_str(), "gel-cli" | "edgedb-cli") {
        return validate_cli(index_source, artifact_base, platform, package).map(Some);
    }
    if package.tags.contains_key("extension") {
        return validate_extension(index_source, artifact_base, package).map(Some);
    }
    Ok(None)
}

fn validate_server(
    index_source: &Source,
    artifact_base: &Source,
    package: &PackageData,
) -> Result<ValidatedArtifact, IndexValidationError> {
    let installref = select_installref(index_source, package, |installref| {
        installref.kind == "application/x-tar" && installref.encoding.as_deref() == Some("zstd")
    })?;
    let version = package.version.parse::<ver::Build>().map_err(|error| {
        IndexValidationError::new(index_source, package, "version", error.to_string())
    })?;
    let url = resolve_installref_url(artifact_base, &installref.path).ok_or_else(|| {
        IndexValidationError::new(
            index_source,
            package,
            "installrefs.ref",
            format!("invalid artifact URL {}", installref.path),
        )
    })?;
    let hash = parse_installref_digest(index_source, package, installref)?;
    let identity = ArtifactIdentity::Server {
        name: package.basename.clone().into_boxed_str(),
        version: package.version.clone().into_boxed_str(),
        slot: package.slot.clone().into_boxed_str(),
    };
    Ok(ValidatedArtifact::Server(
        identity,
        ServerPackage {
            name: package.basename.clone(),
            version,
            url,
            size: installref.verification.size,
            hash,
            kind: PackageType::TarZst,
            slot: package.slot.clone(),
            tags: package.tags.clone(),
        },
    ))
}

fn validate_cli(
    index_source: &Source,
    artifact_base: &Source,
    platform: &str,
    package: &PackageData,
) -> Result<ValidatedArtifact, IndexValidationError> {
    let media_type = cli_media_type(platform).ok_or_else(|| {
        IndexValidationError::new(
            index_source,
            package,
            "installrefs.type",
            format!("no CLI artifact media type for platform {platform}"),
        )
    })?;
    let installref = select_installref(index_source, package, |installref| {
        installref.kind == media_type && installref.encoding.as_deref() == Some("zstd")
    })
    .or_else(|_| {
        if package.installrefs.is_empty() {
            Err(IndexValidationError::new(
                index_source,
                package,
                "installrefs",
                "package has no installrefs",
            ))
        } else {
            package
                .installrefs
                .iter()
                .find(|installref| {
                    installref.kind == media_type
                        && installref.encoding.as_deref() == Some("identity")
                })
                .ok_or_else(|| {
                    IndexValidationError::new(
                        index_source,
                        package,
                        "installrefs.type",
                        format!("no {media_type} CLI installref with zstd or identity encoding"),
                    )
                })
        }
    })?;
    let version = package.version.parse::<ver::Semver>().map_err(|error| {
        IndexValidationError::new(index_source, package, "version", error.to_string())
    })?;
    let url = resolve_installref_url(artifact_base, &installref.path).ok_or_else(|| {
        IndexValidationError::new(
            index_source,
            package,
            "installrefs.ref",
            format!("invalid artifact URL {}", installref.path),
        )
    })?;
    let hash = parse_installref_digest(index_source, package, installref)?;
    let identity = ArtifactIdentity::Cli {
        name: package.basename.clone().into_boxed_str(),
        version: package.version.clone().into_boxed_str(),
    };
    Ok(ValidatedArtifact::Cli(
        identity,
        CliPackage {
            version,
            url,
            size: installref.verification.size,
            hash,
            compression: (installref.encoding.as_deref() == Some("zstd"))
                .then_some(Compression::Zstd),
        },
    ))
}

fn validate_extension(
    index_source: &Source,
    artifact_base: &Source,
    package: &PackageData,
) -> Result<ValidatedArtifact, IndexValidationError> {
    let extension_name = package
        .tags
        .get("extension")
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            IndexValidationError::new(
                index_source,
                package,
                "tags.extension",
                "extension tag is missing or empty",
            )
        })?;
    let server_slot = package
        .tags
        .get("server_slot")
        .filter(|slot| !slot.is_empty())
        .ok_or_else(|| {
            IndexValidationError::new(
                index_source,
                package,
                "tags.server_slot",
                "server_slot tag is missing or empty",
            )
        })?;
    let installref = select_installref(index_source, package, |installref| {
        installref.kind == "application/zip" && installref.encoding.as_deref() == Some("identity")
    })?;
    let version = package.version.parse::<ver::Build>().map_err(|error| {
        IndexValidationError::new(index_source, package, "version", error.to_string())
    })?;
    let url = resolve_installref_url(artifact_base, &installref.path).ok_or_else(|| {
        IndexValidationError::new(
            index_source,
            package,
            "installrefs.ref",
            format!("invalid artifact URL {}", installref.path),
        )
    })?;
    let hash = parse_installref_digest(index_source, package, installref)?;
    let identity = ArtifactIdentity::Extension {
        name: extension_name.clone().into_boxed_str(),
        version: package.version.clone().into_boxed_str(),
        server_slot: server_slot.clone().into_boxed_str(),
    };
    Ok(ValidatedArtifact::Extension(
        identity,
        ExtensionPackage {
            name: extension_name.clone(),
            version,
            url,
            size: installref.verification.size,
            hash,
            kind: PackageType::Zip,
            slot: package.slot.clone(),
            tags: package.tags.clone(),
        },
    ))
}

fn select_installref<'a, F>(
    index_source: &Source,
    package: &'a PackageData,
    matches: F,
) -> Result<&'a InstallRef, IndexValidationError>
where
    F: Fn(&InstallRef) -> bool,
{
    if package.installrefs.is_empty() {
        return Err(IndexValidationError::new(
            index_source,
            package,
            "installrefs",
            "package has no installrefs",
        ));
    }
    package
        .installrefs
        .iter()
        .find(|installref| matches(installref))
        .ok_or_else(|| {
            IndexValidationError::new(
                index_source,
                package,
                "installrefs.type",
                "no compatible installref",
            )
        })
}

fn parse_installref_digest(
    index_source: &Source,
    package: &PackageData,
    installref: &InstallRef,
) -> Result<Blake2bDigest, IndexValidationError> {
    installref
        .verification
        .blake2b
        .as_deref()
        .ok_or_else(|| {
            IndexValidationError::new(
                index_source,
                package,
                "installrefs.verification.blake2b",
                "Blake2b digest is missing",
            )
        })?
        .parse::<Blake2bDigest>()
        .map_err(|error| {
            IndexValidationError::new(
                index_source,
                package,
                "installrefs.verification.blake2b",
                error.to_string(),
            )
        })
}

fn resolve_installref_url(artifact_base: &Source, value: &str) -> Option<Url> {
    if let Ok(url) = Url::parse(value) {
        return match url.scheme() {
            "http" | "https" | "file" => Some(url),
            _ => None,
        };
    }

    match artifact_base {
        Source::Http(base) => base.join(value).ok(),
        Source::File(path) => {
            let path = if Path::new(value).is_absolute() {
                Path::new(value).to_path_buf()
            } else {
                path.parent().unwrap_or_else(|| Path::new(".")).join(value)
            };
            Url::from_file_path(path).ok()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portable::registry::source::Source;
    use url::Url;

    fn cli_index(slot: &str, media_type: &str, hash: &str) -> IndexDocument {
        let json = format!(
            r#"{{"packages":[{{"basename":"gel-cli","version":"7.10.2","slot":"{slot}","tags":{{}},"installrefs":[{{"ref":"https://example.com/gel-cli.zst","type":"{media_type}","encoding":"zstd","verification":{{"size":10,"blake2b":"{hash}"}}}}]}}]}}"#
        );
        IndexDocument::from_slice(json.as_bytes()).unwrap()
    }

    #[test]
    fn cli_identity_ignores_irrelevant_slot() {
        let first = cli_index("one", "application/x-pie-executable", HASH);
        let second = cli_index("two", "application/x-pie-executable", HASH);
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let first = first
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap();
        let second = second
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap();
        assert_eq!(
            first.artifacts[0].identity(),
            second.artifacts[0].identity()
        );
    }

    #[test]
    fn uppercase_digest_normalizes_to_valid_artifact() {
        let index = cli_index("", "application/x-pie-executable", &HASH.to_uppercase());
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let validated = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap();
        assert!(matches!(
            &validated.artifacts[0],
            ValidatedArtifact::Cli(_, package) if package.hash.to_string() == HASH
        ));
    }

    #[test]
    fn malformed_supported_digest_rejects_index() {
        let index = cli_index("", "application/x-pie-executable", "bad");
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "installrefs.verification.blake2b");
        assert!(error.to_string().contains("gel-cli"));
        assert!(error.to_string().contains("https://example.com/index.json"));
    }

    #[test]
    fn wrong_platform_cli_media_type_rejects_index() {
        let index = cli_index("", "application/x-dosexec", HASH);
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "installrefs.type");
    }

    #[test]
    fn unrelated_malformed_package_is_ignored() {
        let index = IndexDocument::from_slice(
            br#"{"packages":[{"basename":"apt-package","version":"bad","slot":"","tags":{},"installrefs":[]}] }"#,
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let validated = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap();
        assert!(validated.artifacts.is_empty());
    }

    #[test]
    fn server_missing_digest_rejects_index() {
        let index = IndexDocument::from_slice(
            br#"{"packages":[{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{},"installrefs":[{"ref":"https://example.com/server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{"size":10,"blake2b":null}}]}]}"#,
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "installrefs.verification.blake2b");
    }

    #[test]
    fn server_invalid_version_rejects_index() {
        let index = IndexDocument::from_slice(
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"bad","slot":"7","tags":{{}},"installrefs":[{{"ref":"https://example.com/server.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "version");
    }

    #[test]
    fn extension_missing_server_slot_rejects_index() {
        let index = IndexDocument::from_slice(
            format!(
                r#"{{"packages":[{{"basename":"extension-package","version":"7.0+abcdef0","slot":"7","tags":{{"extension":"pgvector"}},"installrefs":[{{"ref":"https://example.com/extension.zip","type":"application/zip","encoding":"identity","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "tags.server_slot");
    }

    #[test]
    fn extension_invalid_artifact_url_rejects_index() {
        let index = IndexDocument::from_slice(
            format!(
                r#"{{"packages":[{{"basename":"extension-package","version":"7.0+abcdef0","slot":"7","tags":{{"extension":"pgvector","server_slot":"7"}},"installrefs":[{{"ref":"gopher://example.com/extension.zip","type":"application/zip","encoding":"identity","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "installrefs.ref");
    }

    #[test]
    fn empty_index_validates() {
        let index = IndexDocument::from_slice(br#"{"packages":[]}"#).unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let validated = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap();
        assert!(validated.artifacts.is_empty());
    }

    #[test]
    fn server_without_installrefs_rejects_index() {
        let index = IndexDocument::from_slice(
            format!(
                r#"{{"packages":[{{"basename":"gel-server","version":"7.0+abcdef0","slot":"7","tags":{{}},"installrefs":[]}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "installrefs");
    }

    #[test]
    fn extension_empty_name_rejects_index() {
        let index = IndexDocument::from_slice(
            format!(
                r#"{{"packages":[{{"basename":"extension-package","version":"7.0+abcdef0","slot":"7","tags":{{"extension":"","server_slot":"7"}},"installrefs":[{{"ref":"https://example.com/extension.zip","type":"application/zip","encoding":"identity","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
            )
            .as_bytes(),
        )
        .unwrap();
        let source = Source::Http(Url::parse("https://example.com/index.json").unwrap());
        let error = index
            .validate(&source, &source, "x86_64-unknown-linux-musl")
            .unwrap_err();
        assert_eq!(error.field, "tags.extension");
    }

    const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn parses_empty_index() {
        let index = IndexDocument::from_slice(br#"{"packages":[]}"#).unwrap();
        assert!(index.packages.is_empty());
    }

    #[test]
    fn parses_blake2b_verification() {
        let json = format!(
            r#"{{"packages":[{{"basename":"gel-server","version":"7.0","slot":"7","tags":{{}},"installrefs":[{{"ref":"/pkg.tar.zst","type":"application/x-tar","encoding":"zstd","verification":{{"size":10,"blake2b":"{HASH}"}}}}]}}]}}"#
        );
        let index = IndexDocument::from_slice(json.as_bytes()).unwrap();
        assert_eq!(
            index.packages[0].installrefs[0]
                .verification
                .blake2b
                .as_deref(),
            Some(HASH)
        );
    }
}
