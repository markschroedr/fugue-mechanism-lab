use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::catalog::valid_agent;

const MAX_SNAPSHOT_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObserverSnapshot {
    instance_id: String,
    captured_at: String,
    overview: Value,
    agents_by_address: HashMap<String, Value>,
}

impl ObserverSnapshot {
    pub fn load(package: &Path, expected_instance: &str) -> anyhow::Result<Self> {
        let evidence = regular_directory(&package.join("evidence"), "evidence directory")?;
        let path = evidence.join("observer.json");
        let metadata = std::fs::symlink_metadata(&path).context("observer snapshot is missing")?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("observer snapshot must be a regular file");
        }
        if metadata.len() > MAX_SNAPSHOT_BYTES {
            bail!("observer snapshot exceeds the size limit");
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        std::fs::File::open(&path)?
            .take(MAX_SNAPSHOT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SNAPSHOT_BYTES {
            bail!("observer snapshot exceeds the size limit");
        }
        let snapshot: Self =
            serde_json::from_slice(&bytes).context("observer snapshot is malformed")?;
        if snapshot.instance_id != expected_instance {
            bail!("observer snapshot instance does not match experiment");
        }
        if snapshot.captured_at.trim().is_empty() {
            bail!("observer snapshot capture time is missing");
        }
        if !snapshot.overview.is_object() {
            bail!("observer snapshot overview must be an object");
        }
        if snapshot
            .agents_by_address
            .iter()
            .any(|(address, detail)| !valid_agent(address) || !detail.is_object())
        {
            bail!("observer snapshot contains an invalid agent entry");
        }
        Ok(snapshot)
    }

    pub fn overview(&self) -> Value {
        self.overview.clone()
    }

    pub fn agent(&self, address: &str) -> anyhow::Result<Value> {
        if !valid_agent(address) {
            bail!("invalid agent address");
        }
        self.agents_by_address
            .get(address)
            .cloned()
            .context("agent is not present in observer snapshot")
    }
}

fn regular_directory(path: &Path, label: &str) -> anyhow::Result<PathBuf> {
    let metadata =
        std::fs::symlink_metadata(path).with_context(|| format!("{label} is missing"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("{label} must be a real directory");
    }
    Ok(path.to_path_buf())
}
