//! Serde models for Mojang's launcher metadata: version manifest, version JSON, asset index, Java runtime manifests.

use serde::Deserialize;
use std::collections::HashMap;

#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    pub versions: Vec<ManifestVersion>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ManifestVersion {
    pub id: String,
    pub url: String,
    pub sha1: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct VersionJson {
    pub id: String,
    #[serde(rename = "mainClass")]
    pub main_class: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(rename = "assetIndex")]
    pub asset_index: AssetIndexRef,
    pub downloads: VersionDownloads,
    #[serde(rename = "javaVersion")]
    pub java_version: JavaVersion,
    pub libraries: Vec<Library>,
    pub arguments: Arguments,
    pub logging: Option<Logging>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct AssetIndexRef {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct VersionDownloads {
    pub client: FileRef,
    pub server: Option<FileRef>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct FileRef {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct JavaVersion {
    pub component: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Library {
    pub name: String,
    pub downloads: Option<LibraryDownloads>,
    pub rules: Option<Vec<Rule>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LibraryDownloads {
    pub artifact: Option<Artifact>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Artifact {
    pub path: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Arguments {
    pub game: Vec<Arg>,
    pub jvm: Vec<Arg>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Arg {
    Plain(String),
    Ruled { rules: Vec<Rule>, value: OneOrMany },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize, Clone, Debug)]
pub struct Rule {
    pub action: String,
    pub os: Option<OsRule>,
    pub features: Option<HashMap<String, bool>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct OsRule {
    pub name: Option<String>,
    pub arch: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Logging {
    pub client: Option<LoggingClient>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LoggingClient {
    pub argument: String,
    pub file: LoggingFile,
}

#[derive(Deserialize, Clone, Debug)]
pub struct LoggingFile {
    pub id: String,
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Deserialize, Debug)]
pub struct AssetIndex {
    pub objects: HashMap<String, AssetObject>,
}

#[derive(Deserialize, Debug)]
pub struct AssetObject {
    pub hash: String,
    pub size: u64,
}

#[derive(Deserialize, Debug)]
pub struct RuntimeAll(pub HashMap<String, HashMap<String, Vec<RuntimeEntry>>>);

#[derive(Deserialize, Debug)]
pub struct RuntimeEntry {
    pub manifest: FileRef,
}

#[derive(Deserialize, Debug)]
pub struct RuntimeManifest {
    pub files: HashMap<String, RuntimeFile>,
}

#[derive(Deserialize, Debug)]
pub struct RuntimeFile {
    #[serde(rename = "type")]
    pub kind: String,
    pub downloads: Option<RuntimeDownloads>,
}

#[derive(Deserialize, Debug)]
pub struct RuntimeDownloads {
    pub raw: FileRef,
}

/// Decides whether a rule list allows an item on this machine (Windows x64, no optional launcher features).
pub fn rules_allow(rules: &Option<Vec<Rule>>) -> bool {
    let Some(rules) = rules else { return true };
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for r in rules {
        let os_ok = r.os.as_ref().is_none_or(|o| {
            o.name.as_deref().is_none_or(|n| n == "windows") && o.arch.as_deref().is_none_or(|a| a == "x86_64")
        });
        let features_ok = r
            .features
            .as_ref()
            .is_none_or(|f| f.values().all(|wanted| !*wanted));
        if os_ok && features_ok {
            allowed = r.action == "allow";
        }
    }
    allowed
}
