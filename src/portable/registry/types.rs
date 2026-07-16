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
    pub hash: PackageHash,
    pub kind: PackageType,
    pub slot: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct CliPackage {
    pub version: ver::Semver,
    pub url: url::Url,
    pub size: u64,
    pub hash: PackageHash,
    pub compression: Option<Compression>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtensionPackage {
    pub name: String,
    pub version: ver::Build,
    pub url: url::Url,
    pub size: u64,
    pub hash: PackageHash,
    pub kind: PackageType,
    pub slot: String,
    pub tags: HashMap<String, String>,
}

#[derive(Debug, Clone)]
pub enum PackageHash {
    Blake2b(Box<str>),
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
        // TODO(tailhook) use package hash when that is available
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

impl PackageHash {
    pub(crate) fn short(&self) -> &str {
        let value = match self {
            PackageHash::Blake2b(value) => value.as_ref(),
            PackageHash::Unknown(value) => value
                .split_once(':')
                .map_or(value.as_ref(), |(_, digest)| digest),
        };
        let end = value
            .char_indices()
            .nth(7)
            .map_or(value.len(), |(idx, _)| idx);
        &value[..end]
    }
}

impl fmt::Display for PackageHash {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PackageHash::Blake2b(val) => write!(f, "blake2b:{val}"),
            PackageHash::Unknown(val) => write!(f, "{val}"),
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
        let s: String = Deserialize::deserialize(deserializer)?;
        if let Some(hash) = s.strip_prefix("blake2b:") {
            if hash.len() != 128 {
                return Err(de::Error::custom("invalid blake2b hash length"));
            }
            return Ok(PackageHash::Blake2b(hash.into()));
        }
        Ok(PackageHash::Unknown(s.into()))
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
    fn short_hash_handles_blake2b() {
        let hash = PackageHash::Blake2b("abcdef123456".into());
        assert_eq!(hash.short(), "abcdef1");
    }

    #[test]
    fn short_hash_handles_unknown_with_prefix() {
        let hash = PackageHash::Unknown("sha256:abcdef123456".into());
        assert_eq!(hash.short(), "abcdef1");
    }

    #[test]
    fn short_hash_handles_unknown_long_prefix() {
        let hash = PackageHash::Unknown("sha512-256:abcdef123456".into());
        assert_eq!(hash.short(), "abcdef1");
    }

    #[test]
    fn short_hash_handles_short_values() {
        let hash = PackageHash::Unknown("sha256:abc".into());
        assert_eq!(hash.short(), "abc");
    }
}
