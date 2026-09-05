use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Clone)]
pub struct HostConfig {
    pub experiments_dir: PathBuf,
    pub gateways_dir: PathBuf,
}

#[derive(Debug, Deserialize)]
struct HostFile {
    experiments_dir: PathBuf,
    gateways_dir: PathBuf,
    #[allow(dead_code)]
    logs_dir: Option<PathBuf>,
    #[allow(dead_code)]
    sandbox_bin: Option<PathBuf>,
    #[allow(dead_code)]
    server_bin: Option<PathBuf>,
    #[allow(dead_code)]
    caddy_bin: Option<PathBuf>,
}

impl HostConfig {
    pub fn load() -> anyhow::Result<Self> {
        let path = std::env::var_os("FUGUE_HOST_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
                    .join(".config/fugue/host.toml")
            });
        Self::from_path(&path)
    }

    pub fn from_path(path: &Path) -> anyhow::Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read host config {}", path.display()))?;
        let parsed: HostFile = toml::from_str(&source)
            .with_context(|| format!("failed to parse host config {}", path.display()))?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let experiments_dir =
            canonical_root(&resolve(&parsed.experiments_dir, base), "experiment")?;
        let gateways_dir = canonical_root(&resolve(&parsed.gateways_dir, base), "gateway")?;
        if experiments_dir.starts_with(&gateways_dir) || gateways_dir.starts_with(&experiments_dir)
        {
            bail!("experiment and gateway roots must not overlap");
        }
        Ok(Self {
            experiments_dir,
            gateways_dir,
        })
    }
}

fn resolve(path: &Path, base: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn canonical_root(path: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("{name} root does not exist: {}", path.display()))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("{name} root must be a real directory");
    }
    path.canonicalize()
        .with_context(|| format!("failed to canonicalize {name} root"))
}
