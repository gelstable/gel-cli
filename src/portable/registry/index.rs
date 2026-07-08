//! Package index parsing and artifact normalization.

use std::collections::HashMap;

use serde::Deserialize;

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

impl IndexDocument {
    pub fn from_slice(bytes: &[u8]) -> anyhow::Result<IndexDocument> {
        let jd = &mut serde_json::Deserializer::from_slice(bytes);
        Ok(serde_path_to_error::deserialize(jd)?)
    }
}

pub fn valid_blake2b(val: &str) -> bool {
    val.len() == 128 && hex::decode(val).map(|x| x.len() == 64).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn validates_blake2b_shape() {
        assert!(valid_blake2b(HASH));
        assert!(!valid_blake2b("abc"));
    }
}
