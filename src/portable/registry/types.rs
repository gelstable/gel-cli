//! Shared package discovery domain types.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize, de, ser};

use crate::portable::ver::{self, FilterMinor, MinorVersion};
use crate::process::IntoArg;

#[derive(Debug, PartialEq, Eq, Clone, Copy, clap::ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    Stable,
    Testing,
    Nightly,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum PackageType {
    TarZst,
    Zip,
}

#[derive(Debug, Clone, serde::Serialize)]
pub enum Compression {
    Zstd,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub channel: Channel,
    pub version: Option<ver::Filter>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QuerySelector<'a> {
    Channel(Channel),
    Version(&'a ver::Filter),
}

impl<'a> QuerySelector<'a> {
    pub fn from_install_flags(
        version: Option<&'a ver::Filter>,
        channel: Option<Channel>,
        nightly: bool,
    ) -> Option<QuerySelector<'a>> {
        if let Some(version) = version {
            Some(QuerySelector::Version(version))
        } else if let Some(channel) = channel {
            Some(QuerySelector::Channel(channel))
        } else if nightly {
            Some(QuerySelector::Channel(Channel::Nightly))
        } else {
            None
        }
    }

    pub fn from_upgrade_flags(
        version: Option<&'a ver::Filter>,
        channel: Option<Channel>,
        nightly: bool,
        testing: bool,
        latest: bool,
    ) -> Option<QuerySelector<'a>> {
        if let Some(version) = version {
            Some(QuerySelector::Version(version))
        } else if let Some(channel) = channel {
            Some(QuerySelector::Channel(channel))
        } else if nightly {
            Some(QuerySelector::Channel(Channel::Nightly))
        } else if testing {
            Some(QuerySelector::Channel(Channel::Testing))
        } else if latest {
            Some(QuerySelector::Channel(Channel::Stable))
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
pub struct QueryDisplay<'a>(&'a Query);

#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerPackage {
    pub name: String,
    pub version: ver::Build,
    pub url: url::Url,
    pub size: u64,
    pub hash: Blake2bDigest,
    pub kind: PackageType,
    pub slot: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]

pub struct CliPackage {
    pub version: ver::Semver,
    pub url: url::Url,
    pub size: u64,
    pub hash: Blake2bDigest,
    pub compression: Option<Compression>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtensionPackage {
    pub name: String,
    pub version: ver::Build,
    pub url: url::Url,
    pub size: u64,
    pub hash: Blake2bDigest,
    pub kind: PackageType,
    pub slot: String,
    pub tags: HashMap<String, String>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ArtifactIdentity {
    Server {
        name: Box<str>,
        version: Box<str>,
        slot: Box<str>,
    },
    Cli {
        name: Box<str>,
        version: Box<str>,
    },
    Extension {
        name: Box<str>,
        version: Box<str>,
        server_slot: Box<str>,
    },
}

impl fmt::Display for ArtifactIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Server {
                name,
                version,
                slot,
            } => write!(formatter, "server {name}@{version} slot {slot}"),
            Self::Cli { name, version } => write!(formatter, "cli {name}@{version}"),
            Self::Extension {
                name,
                version,
                server_slot,
            } => write!(
                formatter,
                "extension {name}@{version} server slot {server_slot}"
            ),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Blake2bDigest([u8; 64]);

#[derive(Debug, thiserror::Error)]
pub enum DigestParseError {
    #[error("invalid Blake2b digest length {actual}; expected 128 hexadecimal characters")]
    InvalidLength { actual: usize },
    #[error("invalid Blake2b hexadecimal digest: {0}")]
    InvalidHex(#[from] hex::FromHexError),
}

impl Blake2bDigest {
    pub(crate) fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }

    pub(crate) fn short(&self) -> String {
        self.to_string().chars().take(7).collect()
    }
}

impl std::str::FromStr for Blake2bDigest {
    type Err = DigestParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 128 {
            return Err(DigestParseError::InvalidLength {
                actual: value.len(),
            });
        }
        let bytes = hex::decode(value)?;
        let bytes: [u8; 64] =
            bytes
                .try_into()
                .map_err(|bytes: Vec<u8>| DigestParseError::InvalidLength {
                    actual: bytes.len() * 2,
                })?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for Blake2bDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&hex::encode(self.0))
    }
}

impl Serialize for Blake2bDigest {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Blake2bDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageHash {
    Blake2b(Blake2bDigest),
    Unknown(Box<str>),
}

impl PackageType {
    fn as_ext(&self) -> &str {
        match self {
            PackageType::TarZst => ".tar.zst",
            PackageType::Zip => ".zip",
        }
    }
}

impl ServerPackage {
    pub fn cache_file_name(&self) -> String {
        let hash = self.hash.short();
        format!(
            "edgedb-server_{}_{:7}{}",
            self.version,
            hash,
            self.kind.as_ext()
        )
    }
}

impl fmt::Display for ServerPackage {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

impl fmt::Display for PackageHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PackageHash::Blake2b(digest) => write!(formatter, "blake2b:{digest}"),
            PackageHash::Unknown(value) => formatter.write_str(value),
        }
    }
}

impl Query {
    pub fn nightly() -> Query {
        Query {
            channel: Channel::Nightly,
            version: None,
        }
    }

    pub fn stable() -> Query {
        Query {
            channel: Channel::Stable,
            version: None,
        }
    }

    pub fn testing() -> Query {
        Query {
            channel: Channel::Testing,
            version: None,
        }
    }

    pub fn display(&self) -> QueryDisplay {
        QueryDisplay(self)
    }

    pub fn from_selector(
        selector: Option<QuerySelector<'_>>,
        default: impl FnOnce() -> anyhow::Result<Query>,
    ) -> anyhow::Result<(Query, bool)> {
        match selector {
            Some(QuerySelector::Version(ver)) => Ok((Query::from_filter(ver)?, true)),
            Some(QuerySelector::Channel(channel)) => Ok((
                Query {
                    channel,
                    version: None,
                },
                true,
            )),
            None => Ok((default()?, false)),
        }
    }

    pub fn from_filter(ver: &ver::Filter) -> anyhow::Result<Query> {
        match ver.minor {
            None => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver.clone()),
            }),
            Some(FilterMinor::Dev(_)) => Ok(Query {
                channel: Channel::Nightly,
                version: Some(ver.clone()),
            }),
            Some(FilterMinor::Alpha(_)) | Some(FilterMinor::Beta(_)) | Some(FilterMinor::Rc(_))
                if ver.major == 1 || ver.major == 2 =>
            {
                Ok(Query {
                    channel: Channel::Stable,
                    version: Some(ver.clone()),
                })
            }
            Some(FilterMinor::Alpha(_)) | Some(FilterMinor::Beta(_)) | Some(FilterMinor::Rc(_)) => {
                Ok(Query {
                    channel: Channel::Testing,
                    version: Some(ver.clone()),
                })
            }
            Some(FilterMinor::Minor(_)) => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver.clone()),
            }),
        }
    }

    pub fn from_version(ver: &ver::Specific) -> anyhow::Result<Query> {
        match ver.minor {
            MinorVersion::Dev(v) => Ok(Query {
                channel: Channel::Nightly,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Minor(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Alpha(v) if ver.major == 1 => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Alpha(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Beta(v) if ver.major == 1 => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Beta(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Rc(v) if ver.major == 1 || ver.major == 2 => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Rc(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Minor(v) => Ok(Query {
                channel: Channel::Stable,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Minor(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Alpha(v) => Ok(Query {
                channel: Channel::Testing,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Alpha(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Beta(v) => Ok(Query {
                channel: Channel::Testing,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Beta(v)),
                    exact: false,
                }),
            }),
            MinorVersion::Rc(v) => Ok(Query {
                channel: Channel::Testing,
                version: Some(ver::Filter {
                    major: ver.major,
                    minor: Some(FilterMinor::Rc(v)),
                    exact: false,
                }),
            }),
        }
    }

    pub fn matches(&self, ver: &ver::Build) -> bool {
        match &self.version {
            Some(query_ver) => query_ver.matches(ver),
            None => Channel::from_version(&ver.specific())
                .map(|channel| self.channel == channel)
                .unwrap_or(false),
        }
    }

    pub fn as_config_value(&self) -> String {
        if let Some(ver) = &self.version {
            ver.to_string()
        } else if self.channel == Channel::Nightly {
            "nightly".into()
        } else {
            "*".into()
        }
    }

    pub fn is_nightly(&self) -> bool {
        matches!(self.channel, Channel::Nightly)
    }

    pub fn is_nonrecursive_access_policies_needed(&self) -> bool {
        self.version
            .as_ref()
            .map(|f| match (f.major, f.minor) {
                (1, _) => false,
                (2, Some(v)) if v < FilterMinor::Minor(6) => false,
                (2, _) => true,
                _ => false,
            })
            .unwrap_or(false)
    }

    pub fn is_simple_scoping_needed(&self) -> bool {
        self.version.as_ref().map(|f| f.major == 6).unwrap_or(false)
    }

    pub fn is_no_linkful_computed_splats_needed(&self) -> bool {
        self.version.as_ref().map(|f| f.major == 7).unwrap_or(false)
    }

    pub fn has_ext_auth(&self) -> bool {
        self.version.as_ref().map(|f| f.major >= 4).unwrap_or(true)
    }

    pub fn has_ext_ai(&self) -> bool {
        self.version.as_ref().map(|f| f.major >= 5).unwrap_or(true)
    }

    pub fn has_ext_postgis(&self) -> bool {
        self.version.as_ref().map(|f| f.major >= 6).unwrap_or(false)
    }

    pub fn cli_channel(&self) -> Option<Channel> {
        // Only one argument in CLI is allowed
        // So we skip channel if version is set, since version unambiguously
        // determines the channel. But if there is no version we must provide
        // channel.
        if self.version.is_some() {
            None
        } else {
            Some(self.channel)
        }
    }
}

impl Serialize for PackageHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl Serialize for Channel {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: ser::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PackageHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.strip_prefix("blake2b:") {
            Some(digest) => digest
                .parse()
                .map(PackageHash::Blake2b)
                .map_err(de::Error::custom),
            None => Ok(PackageHash::Unknown(value.into_boxed_str())),
        }
    }
}

impl std::str::FromStr for Query {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Query> {
        if s == "*" {
            Ok(Query {
                channel: Channel::Stable,
                version: None,
            })
        } else if s == "nightly" {
            Ok(Query {
                channel: Channel::Nightly,
                version: None,
            })
        } else {
            let ver: ver::Filter = s.parse()?;
            Ok(Query {
                channel: Channel::from_filter(&ver)?,
                version: Some(ver),
            })
        }
    }
}

impl From<Channel> for Query {
    fn from(channel: Channel) -> Query {
        Query {
            channel,
            version: None,
        }
    }
}

impl<'de> Deserialize<'de> for Query {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: de::Deserializer<'de>,
    {
        let s: String = Deserialize::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Channel {
    pub fn from_version(ver: &ver::Specific) -> anyhow::Result<Channel> {
        use ver::MinorVersion::*;

        match ver.minor {
            Dev(_) => Ok(Channel::Nightly),
            Minor(_) => Ok(Channel::Stable),
            Alpha(_) | Beta(_) | Rc(_) if ver.major == 1 || ver.major == 2 => {
                // before 2.0 all prereleases go into a stable channel
                Ok(Channel::Stable)
            }
            Alpha(_) => Ok(Channel::Testing),
            Beta(_) => Ok(Channel::Testing),
            Rc(_) => Ok(Channel::Testing),
        }
    }

    pub fn from_filter(ver: &ver::Filter) -> anyhow::Result<Channel> {
        use ver::FilterMinor::*;

        match ver.minor {
            None => Ok(Channel::Stable),
            Some(Minor(_)) => Ok(Channel::Stable),
            Some(Alpha(_) | Beta(_) | Rc(_)) if ver.major == 1 || ver.major == 2 => {
                // before 1.0 all prereleases go into a stable channel
                Ok(Channel::Stable)
            }
            Some(Alpha(_) | Beta(_) | Rc(_)) => Ok(Channel::Testing),
            Some(Dev(_)) => Ok(Channel::Nightly),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Channel::Nightly => "nightly",
            Channel::Stable => "stable",
            Channel::Testing => "testing",
        }
    }
}

impl IntoArg for &Channel {
    fn add_arg(self, process: &mut crate::process::Native) {
        process.arg(self.as_str());
    }
}

impl fmt::Display for QueryDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        use ver::FilterMinor::*;

        match &self.0.version {
            None => self.0.channel.as_str().fmt(f),
            Some(ver) => {
                ver.major.fmt(f)?;
                f.write_str(".")?;
                match ver.minor {
                    None => "0".fmt(f),
                    Some(Minor(m)) => m.fmt(f),
                    Some(Dev(v)) => write!(f, "0-dev.{v}"),
                    Some(Alpha(v)) => write!(f, "0-alpha.{v}"),
                    Some(Beta(v)) => write!(f, "0-beta.{v}"),
                    Some(Rc(v)) => write!(f, "0-rc.{v}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_query() -> anyhow::Result<Query> {
        Ok(Query::stable())
    }

    #[test]
    fn selector_none_uses_default_and_is_not_explicit() {
        let (query, explicit) = Query::from_selector(None, default_query).unwrap();

        assert_eq!(query, Query::stable());
        assert!(!explicit);
    }

    #[test]
    fn selector_channel_stable_is_explicit() {
        let (query, explicit) =
            Query::from_selector(Some(QuerySelector::Channel(Channel::Stable)), default_query)
                .unwrap();

        assert_eq!(query, Query::stable());
        assert!(explicit);
    }

    #[test]
    fn selector_channel_testing_is_explicit() {
        let (query, explicit) = Query::from_selector(
            Some(QuerySelector::Channel(Channel::Testing)),
            default_query,
        )
        .unwrap();

        assert_eq!(query, Query::testing());
        assert!(explicit);
    }

    #[test]
    fn selector_channel_nightly_is_explicit() {
        let (query, explicit) = Query::from_selector(
            Some(QuerySelector::Channel(Channel::Nightly)),
            default_query,
        )
        .unwrap();

        assert_eq!(query, Query::nightly());
        assert!(explicit);
    }

    #[test]
    fn selector_version_delegates_to_filter_and_is_explicit() {
        let filter = ver::Filter {
            major: 7,
            minor: Some(FilterMinor::Dev(1)),
            exact: false,
        };

        let (query, explicit) =
            Query::from_selector(Some(QuerySelector::Version(&filter)), default_query).unwrap();

        assert_eq!(query, Query::from_filter(&filter).unwrap());
        assert!(explicit);
    }
    #[test]
    fn install_selector_precedence_is_table_driven() {
        let filter = ver::Filter {
            major: 7,
            minor: Some(FilterMinor::Dev(1)),
            exact: false,
        };
        let cases = [
            (
                Some(&filter),
                Some(Channel::Testing),
                true,
                Some(QuerySelector::Version(&filter)),
            ),
            (
                None,
                Some(Channel::Testing),
                true,
                Some(QuerySelector::Channel(Channel::Testing)),
            ),
            (
                None,
                None,
                true,
                Some(QuerySelector::Channel(Channel::Nightly)),
            ),
            (None, None, false, None),
        ];

        for (version, channel, nightly, expected) in cases {
            assert_eq!(
                QuerySelector::from_install_flags(version, channel, nightly),
                expected
            );
        }
    }

    #[test]
    fn upgrade_selector_precedence_is_table_driven() {
        let filter = ver::Filter {
            major: 7,
            minor: Some(FilterMinor::Dev(1)),
            exact: false,
        };
        let cases = [
            (
                Some(&filter),
                Some(Channel::Testing),
                true,
                true,
                true,
                Some(QuerySelector::Version(&filter)),
            ),
            (
                None,
                Some(Channel::Testing),
                true,
                true,
                true,
                Some(QuerySelector::Channel(Channel::Testing)),
            ),
            (
                None,
                None,
                true,
                true,
                true,
                Some(QuerySelector::Channel(Channel::Nightly)),
            ),
            (
                None,
                None,
                false,
                true,
                true,
                Some(QuerySelector::Channel(Channel::Testing)),
            ),
            (
                None,
                None,
                false,
                false,
                true,
                Some(QuerySelector::Channel(Channel::Stable)),
            ),
            (None, None, false, false, false, None),
        ];

        for (version, channel, nightly, testing, latest, expected) in cases {
            assert_eq!(
                QuerySelector::from_upgrade_flags(version, channel, nightly, testing, latest),
                expected
            );
        }
    }

    #[test]
    fn blake2b_digest_accepts_upper_and_lower_hex() {
        let lower = "ab".repeat(64).parse::<Blake2bDigest>().unwrap();
        let upper = "AB".repeat(64).parse::<Blake2bDigest>().unwrap();
        assert_eq!(lower, upper);
        assert_eq!(lower.to_string(), "ab".repeat(64));
    }

    #[test]
    fn blake2b_digest_rejects_wrong_length() {
        let error = "ab".repeat(63).parse::<Blake2bDigest>().unwrap_err();
        assert!(matches!(
            error,
            DigestParseError::InvalidLength { actual: 126 }
        ));
    }

    #[test]
    fn blake2b_digest_rejects_non_hex() {
        let error = format!("{}zz", "ab".repeat(63))
            .parse::<Blake2bDigest>()
            .unwrap_err();
        assert!(matches!(error, DigestParseError::InvalidHex(_)));
    }

    #[test]
    fn package_hash_round_trips_canonical_blake2b() {
        let digest = "AB".repeat(64).parse::<Blake2bDigest>().unwrap();
        let value = PackageHash::Blake2b(digest);
        let json = serde_json::to_string(&value).unwrap();
        assert_eq!(json, format!("\"blake2b:{}\"", "ab".repeat(64)));
        assert_eq!(serde_json::from_str::<PackageHash>(&json).unwrap(), value);
    }
    #[test]
    fn artifact_identity_contains_only_kind_specific_fields() {
        let server_seven = ArtifactIdentity::Server {
            name: "gel-server".into(),
            version: "7.0+abcdef0".into(),
            slot: "7".into(),
        };
        let server_eight = ArtifactIdentity::Server {
            name: "gel-server".into(),
            version: "7.0+abcdef0".into(),
            slot: "8".into(),
        };
        let cli = ArtifactIdentity::Cli {
            name: "gel-cli".into(),
            version: "7.10.2".into(),
        };
        let extension_seven = ArtifactIdentity::Extension {
            name: "pgvector".into(),
            version: "7.0+abcdef0".into(),
            server_slot: "7".into(),
        };
        let extension_eight = ArtifactIdentity::Extension {
            name: "pgvector".into(),
            version: "7.0+abcdef0".into(),
            server_slot: "8".into(),
        };
        assert_ne!(server_seven, server_eight);
        assert_eq!(
            cli,
            ArtifactIdentity::Cli {
                name: "gel-cli".into(),
                version: "7.10.2".into(),
            }
        );
        assert_ne!(extension_seven, extension_eight);
    }
}
