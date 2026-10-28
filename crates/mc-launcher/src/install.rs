//! Downloads everything a version needs (client jar, libraries, assets, Java runtime) with SHA-1 checks and shared progress.

use crate::meta::*;
use anyhow::{Context, Result, bail};
use sha1::{Digest, Sha1};
use std::collections::VecDeque;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const RUNTIME_URL: &str = "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";
const ASSET_URL: &str = "https://resources.download.minecraft.net";
const RUNTIME_PLATFORM: &str = "windows-x64";
const WORKERS: usize = 12;

#[derive(Clone)]
pub struct Paths {
    pub root: PathBuf,
}

impl Paths {
    /// `%USERPROFILE%\mcrs\data`. Not under AppData: packaged apps (like the Claude desktop app) get
    /// AppData writes redirected into private storage, which other programs then can't see.
    pub fn new() -> Self {
        let root = directories::BaseDirs::new()
            .map(|d| d.home_dir().join("mcrs").join("data"))
            .unwrap_or_else(|| PathBuf::from("mcrs-data"));
        Self { root }
    }
    pub fn versions(&self) -> PathBuf { self.root.join("versions") }
    pub fn libraries(&self) -> PathBuf { self.root.join("libraries") }
    pub fn assets(&self) -> PathBuf { self.root.join("assets") }
    pub fn runtime(&self) -> PathBuf { self.root.join("runtime") }
    pub fn natives(&self, id: &str) -> PathBuf { self.root.join("natives").join(id) }
    pub fn game(&self) -> PathBuf { self.root.join("game") }
    pub fn version_dir(&self, id: &str) -> PathBuf { self.versions().join(id) }
    pub fn client_jar(&self, id: &str) -> PathBuf { self.version_dir(id).join(format!("{id}.jar")) }
    pub fn version_json(&self, id: &str) -> PathBuf { self.version_dir(id).join(format!("{id}.json")) }
    fn ok_marker(&self, id: &str) -> PathBuf { self.version_dir(id).join(".installed") }
    pub fn is_installed(&self, id: &str) -> bool { self.ok_marker(id).exists() }
    pub fn java_exe(&self, component: &str) -> PathBuf { self.runtime().join(component).join("bin").join("java.exe") }
}

#[derive(Default)]
pub struct Progress {
    pub stage: Mutex<String>,
    pub bytes_done: AtomicU64,
    pub bytes_total: AtomicU64,
    pub files_done: AtomicUsize,
    pub files_total: AtomicUsize,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn set_stage(&self, s: impl Into<String>) {
        *self.stage.lock().unwrap() = s.into();
    }
    pub fn reset_counts(&self) {
        self.bytes_done.store(0, Relaxed);
        self.bytes_total.store(0, Relaxed);
        self.files_done.store(0, Relaxed);
        self.files_total.store(0, Relaxed);
    }
}

pub struct Job {
    url: String,
    dest: PathBuf,
    sha1: Option<String>,
    size: u64,
}

pub struct Prepared {
    pub version: VersionJson,
    pub java: PathBuf,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent("mcrs-launcher/0.1")
        .build()
        .into()
}

fn get_string(url: &str) -> Result<String> {
    let mut resp = agent().get(url).call().with_context(|| format!("GET {url}"))?;
    Ok(resp.body_mut().read_to_string()?)
}

pub fn fetch_manifest() -> Result<Manifest> {
    Ok(serde_json::from_str(&get_string(MANIFEST_URL)?)?)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha1_file(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn is_current(job: &Job) -> bool {
    fs::metadata(&job.dest).is_ok_and(|m| m.len() == job.size)
}

fn download_one(job: &Job, progress: &Progress) -> Result<()> {
    if let Some(dir) = job.dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut part = job.dest.clone().into_os_string();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut last_err = None;
    for _ in 0..3 {
        if progress.cancel.load(Relaxed) {
            bail!("cancelled");
        }
        let mut counted = 0u64;
        let attempt = (|| -> Result<()> {
            let mut resp = agent().get(&job.url).call().with_context(|| format!("GET {}", job.url))?;
            let mut reader = resp.body_mut().as_reader();
            let mut out = fs::File::create(&part)?;
            let mut hasher = Sha1::new();
            let mut buf = vec![0u8; 65536];
            loop {
                if progress.cancel.load(Relaxed) {
                    bail!("cancelled");
                }
                let n = reader.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                hasher.update(&buf[..n]);
                counted += n as u64;
                progress.bytes_done.fetch_add(n as u64, Relaxed);
            }
            out.flush()?;
            if let Some(want) = &job.sha1 {
                let got = hex(&hasher.finalize());
                if &got != want {
                    bail!("hash mismatch for {} (want {want}, got {got})", job.url);
                }
            }
            fs::rename(&part, &job.dest)?;
            Ok(())
        })();
        match attempt {
            Ok(()) => return Ok(()),
            Err(e) => {
                progress.bytes_done.fetch_sub(counted, Relaxed);
                let _ = fs::remove_file(&part);
                if e.to_string() == "cancelled" {
                    return Err(e);
                }
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap())
}

/// Runs the jobs that aren't already on disk across a small worker pool.
fn run_jobs(jobs: Vec<Job>, progress: &Arc<Progress>) -> Result<()> {
    let jobs: Vec<Job> = jobs.into_iter().filter(|j| !is_current(j)).collect();
    progress.reset_counts();
    progress.files_total.store(jobs.len(), Relaxed);
    progress.bytes_total.store(jobs.iter().map(|j| j.size).sum(), Relaxed);
    let queue = Arc::new(Mutex::new(jobs.into_iter().collect::<VecDeque<_>>()));
    let failure: Arc<Mutex<Option<anyhow::Error>>> = Arc::new(Mutex::new(None));
    std::thread::scope(|s| {
        for _ in 0..WORKERS {
            let (queue, failure, progress) = (queue.clone(), failure.clone(), progress.clone());
            s.spawn(move || {
                loop {
                    if failure.lock().unwrap().is_some() {
                        return;
                    }
                    let Some(job) = queue.lock().unwrap().pop_front() else { return };
                    match download_one(&job, &progress) {
                        Ok(()) => {
                            progress.files_done.fetch_add(1, Relaxed);
                        }
                        Err(e) => {
                            failure.lock().unwrap().get_or_insert(e);
                            return;
                        }
                    }
                }
            });
        }
    });
    match Arc::try_unwrap(failure).ok().and_then(|m| m.into_inner().unwrap()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn load_version_json(paths: &Paths, entry: &ManifestVersion) -> Result<VersionJson> {
    let path = paths.version_json(&entry.id);
    let fresh = path.exists() && sha1_file(&path).is_ok_and(|h| h == entry.sha1);
    if !fresh {
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, get_string(&entry.url)?)?;
    }
    Ok(serde_json::from_str(&fs::read_to_string(&path)?)?)
}

pub fn library_jobs(paths: &Paths, v: &VersionJson) -> Vec<Job> {
    v.libraries
        .iter()
        .filter(|l| rules_allow(&l.rules) && !l.name.ends_with("-arm64"))
        .filter_map(|l| l.downloads.as_ref()?.artifact.as_ref())
        .map(|a| Job {
            url: a.url.clone(),
            dest: paths.libraries().join(&a.path),
            sha1: Some(a.sha1.clone()),
            size: a.size,
        })
        .collect()
}

/// Library jar paths in classpath order for the current platform.
pub fn library_jobs_paths(paths: &Paths, v: &VersionJson) -> Vec<PathBuf> {
    library_jobs(paths, v).into_iter().map(|j| j.dest).collect()
}

fn runtime_jobs(paths: &Paths, component: &str) -> Result<Vec<Job>> {
    let all: RuntimeAll = serde_json::from_str(&get_string(RUNTIME_URL)?)?;
    let entry = all
        .0
        .get(RUNTIME_PLATFORM)
        .and_then(|p| p.get(component))
        .and_then(|e| e.first())
        .with_context(|| format!("no {component} runtime for {RUNTIME_PLATFORM}"))?;
    let manifest: RuntimeManifest = serde_json::from_str(&get_string(&entry.manifest.url)?)?;
    let base = paths.runtime().join(component);
    Ok(manifest
        .files
        .into_iter()
        .filter(|(_, f)| f.kind == "file")
        .filter_map(|(rel, f)| {
            let raw = f.downloads?.raw;
            Some(Job { url: raw.url, dest: base.join(rel), sha1: Some(raw.sha1), size: raw.size })
        })
        .collect())
}

fn asset_jobs(paths: &Paths, v: &VersionJson) -> Result<Vec<Job>> {
    let index_path = paths.assets().join("indexes").join(format!("{}.json", v.asset_index.id));
    let want = Job {
        url: v.asset_index.url.clone(),
        dest: index_path.clone(),
        sha1: Some(v.asset_index.sha1.clone()),
        size: v.asset_index.size,
    };
    if !is_current(&want) {
        download_one(&want, &Progress::default())?;
    }
    let index: AssetIndex = serde_json::from_str(&fs::read_to_string(&index_path)?)?;
    Ok(index
        .objects
        .into_values()
        .map(|o| {
            let prefix = &o.hash[..2];
            Job {
                url: format!("{ASSET_URL}/{prefix}/{}", o.hash),
                dest: paths.assets().join("objects").join(prefix).join(&o.hash),
                sha1: Some(o.hash.clone()),
                size: o.size,
            }
        })
        .collect())
}

/// Makes sure `id` is fully downloaded and returns what's needed to launch it.
pub fn prepare(paths: &Paths, id: &str, manifest: Option<&Manifest>, progress: &Arc<Progress>) -> Result<Prepared> {
    if paths.is_installed(id) {
        let v: VersionJson = serde_json::from_str(&fs::read_to_string(paths.version_json(id))?)?;
        let java = paths.java_exe(&v.java_version.component);
        if java.exists() {
            return Ok(Prepared { version: v, java });
        }
    }
    let manifest = manifest.context("version list not loaded (are you offline?)")?;
    let entry = manifest.versions.iter().find(|v| v.id == id).with_context(|| format!("unknown version {id}"))?;

    progress.set_stage("Reading version info");
    let v = load_version_json(paths, entry)?;

    progress.set_stage("Game and libraries");
    let mut jobs = vec![Job {
        url: v.downloads.client.url.clone(),
        dest: paths.client_jar(id),
        sha1: Some(v.downloads.client.sha1.clone()),
        size: v.downloads.client.size,
    }];
    jobs.extend(library_jobs(paths, &v));
    if let Some(f) = v.logging.as_ref().and_then(|l| l.client.as_ref()).map(|c| &c.file) {
        jobs.push(Job {
            url: f.url.clone(),
            dest: paths.assets().join("log_configs").join(&f.id),
            sha1: Some(f.sha1.clone()),
            size: f.size,
        });
    }
    run_jobs(jobs, progress)?;

    progress.set_stage("Java runtime");
    run_jobs(runtime_jobs(paths, &v.java_version.component)?, progress)?;

    progress.set_stage("Game assets");
    run_jobs(asset_jobs(paths, &v)?, progress)?;

    fs::write(paths.ok_marker(id), "")?;
    let java = paths.java_exe(&v.java_version.component);
    Ok(Prepared { version: v, java })
}
