use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::config::HostConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExperimentSummary {
    pub id: String,
    pub state: ExperimentState,
    pub api_reachable: bool,
    pub api_port: u16,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExperimentState {
    Paused,
    Prepared,
    Failed,
    Active,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostBinding {
    pub instance_id: String,
    pub api_host: IpAddr,
    pub api_port: u16,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Health {
    instance_id: String,
}

#[derive(Clone)]
pub struct ExperimentCatalog {
    host: HostConfig,
    client: reqwest::Client,
}

pub struct Experiment {
    pub id: String,
    pub package: PathBuf,
    pub binding: HostBinding,
    pub state: ExperimentState,
}

impl ExperimentCatalog {
    pub fn new(host: HostConfig) -> anyhow::Result<Self> {
        Ok(Self {
            host,
            client: reqwest::Client::builder()
                .timeout(Duration::from_millis(700))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
        })
    }

    pub async fn list(&self) -> anyhow::Result<Vec<ExperimentSummary>> {
        let mut experiments = Vec::new();
        for entry in std::fs::read_dir(&self.host.experiments_dir)
            .context("failed to read experiment root")?
        {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() || file_type.is_symlink() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !valid_id(&id) {
                continue;
            }
            let Ok(experiment) = self.resolve(&id) else {
                continue;
            };
            let api_reachable = matches!(
                experiment.state,
                ExperimentState::Active | ExperimentState::Prepared
            ) && self.verify_instance(&experiment).await.is_ok();
            experiments.push(ExperimentSummary {
                id,
                state: experiment.state,
                api_reachable,
                api_port: experiment.binding.api_port,
            });
        }
        experiments.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(experiments)
    }

    pub fn resolve(&self, id: &str) -> anyhow::Result<Experiment> {
        if !valid_id(id) {
            bail!("invalid experiment id");
        }
        let package = regular_child_directory(&self.host.experiments_dir, id, "experiment")?;
        require_regular_file(&package.join("fugue.toml"), "experiment config")?;

        let gateway = regular_child_directory(&self.host.gateways_dir, id, "gateway binding")?;
        let binding_path = gateway.join("binding.json");
        require_regular_file(&binding_path, "gateway binding")?;
        let source =
            std::fs::read_to_string(&binding_path).context("failed to read gateway binding")?;
        let binding: HostBinding =
            serde_json::from_str(&source).context("failed to parse gateway binding")?;
        validate_binding(id, &binding)?;

        let paused = lifecycle_marker(&package.join(".paused"))?;
        let prepared = lifecycle_marker(&package.join(".prepared"))?;
        let failed = lifecycle_marker(&package.join(".failed"))?;
        if [paused, prepared, failed]
            .into_iter()
            .filter(|present| *present)
            .count()
            > 1
        {
            bail!("experiment has conflicting lifecycle markers");
        }
        let state = if paused {
            ExperimentState::Paused
        } else if prepared {
            ExperimentState::Prepared
        } else if failed {
            ExperimentState::Failed
        } else {
            ExperimentState::Active
        };
        Ok(Experiment {
            id: id.to_owned(),
            package,
            binding,
            state,
        })
    }

    pub async fn verify_instance(&self, experiment: &Experiment) -> anyhow::Result<()> {
        let response = self
            .client
            .get(self.url(experiment, "/api/health"))
            .send()
            .await
            .context("experiment API is unreachable")?;
        if !response.status().is_success() {
            bail!("experiment health check failed");
        }
        let health: Health = response
            .json()
            .await
            .context("experiment health response is invalid")?;
        if health.instance_id != experiment.id {
            bail!("experiment API identity does not match binding");
        }
        Ok(())
    }

    pub fn url(&self, experiment: &Experiment, path: &str) -> String {
        let endpoint = SocketAddr::new(experiment.binding.api_host, experiment.binding.api_port);
        format!("http://{endpoint}{path}")
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

fn validate_binding(id: &str, binding: &HostBinding) -> anyhow::Result<()> {
    if binding.instance_id != id {
        bail!("gateway binding instance does not match experiment");
    }
    if binding.api_host != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        bail!("gateway binding API host must be 127.0.0.1");
    }
    if binding.api_port == 0 {
        bail!("gateway binding API port must be nonzero");
    }
    Ok(())
}

pub fn valid_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    value != "default"
        && (1..=40).contains(&bytes.len())
        && bytes.first().is_some_and(u8::is_ascii_alphanumeric)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub fn valid_agent(value: &str) -> bool {
    (2..=40).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn regular_child_directory(root: &Path, name: &str, label: &str) -> anyhow::Result<PathBuf> {
    let path = root.join(name);
    let metadata =
        std::fs::symlink_metadata(&path).with_context(|| format!("{label} not found"))?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("{label} must be a real directory");
    }
    Ok(path)
}

fn require_regular_file(path: &Path, label: &str) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(path).with_context(|| format!("{label} not found"))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        bail!("{label} must be a regular file");
    }
    Ok(())
}

fn lifecycle_marker(path: &Path) -> anyhow::Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            Ok(true)
        }
        Ok(_) => bail!("experiment lifecycle marker must be a regular file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("failed to inspect experiment lifecycle marker"),
    }
}
