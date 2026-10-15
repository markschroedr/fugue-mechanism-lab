use std::collections::HashMap;
use std::env;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use fugue_store::SocietyComputeBacking;
use serde::{Deserialize, Serialize};

pub(crate) const DEFAULT_CONFIG_PATH: &str = "fugue.toml";
const HOST_API_ADDRESS: Ipv4Addr = Ipv4Addr::LOCALHOST;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ApiListeners {
    pub host: SocketAddr,
    pub guest: Option<SocketAddr>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FugueConfig {
    pub storage: StorageConfig,
    pub api: ApiConfig,
    pub ui: UiConfig,
    pub llm: LlmConfig,
    pub sources: SourcesConfig,
    #[serde(default)]
    pub society: SocietyConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StorageConfig {
    pub data_dir: PathBuf,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApiConfig {
    pub host: IpAddr,
    pub port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UiConfig {
    pub host: IpAddr,
    pub port: u16,
    pub preview_port: u16,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LlmConfig {
    pub model: String,
    pub effort: String,
    pub request_timeout_seconds: u64,
    #[serde(default)]
    pub agents: HashMap<String, AgentLlmOverride>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentLlmOverride {
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourcesConfig {
    pub forum_magnum: ForumMagnumConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ForumMagnumConfig {
    pub endpoint: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SocietyConfig {
    #[serde(default = "default_world")]
    pub world: String,
    /// Operator declarations only; the world does not install these services.
    #[serde(default)]
    pub instrumentation: std::collections::BTreeMap<String, String>,
    #[serde(default = "default_society_instance_id")]
    pub instance_id: String,
    #[serde(default = "default_society_mission")]
    pub mission: String,
    #[serde(default = "default_treasury_address")]
    pub treasury_address: String,
    #[serde(default = "default_weekly_allowance")]
    pub weekly_allowance: i64,
    #[serde(default = "default_compute_backing")]
    pub compute_backing: SocietyComputeBacking,
    #[serde(default)]
    pub shared_daily_line: String,
    #[serde(default)]
    pub guest_api_url: Option<String>,
    #[serde(default = "default_kit_dir")]
    pub kit_dir: PathBuf,
    #[serde(default)]
    pub runner: SocietyRunnerConfig,
    #[serde(default = "default_society_models")]
    pub models: HashMap<String, SocietyModelConfig>,
    /// Host path of the external microvm-sandbox CLI; consumed by the `./fugue`
    /// launcher for keeper services, never by the server itself.
    #[serde(default)]
    #[allow(dead_code)]
    pub sandbox_bin: String,
}

impl Default for SocietyConfig {
    fn default() -> Self {
        Self {
            world: default_world(),
            instrumentation: Default::default(),
            instance_id: default_society_instance_id(),
            mission: default_society_mission(),
            treasury_address: default_treasury_address(),
            weekly_allowance: default_weekly_allowance(),
            compute_backing: default_compute_backing(),
            shared_daily_line: String::new(),
            guest_api_url: None,
            kit_dir: default_kit_dir(),
            runner: SocietyRunnerConfig::default(),
            models: default_society_models(),
            sandbox_bin: String::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)] // Birth tooling consumes the complete runner shape from the same TOML file.
pub(crate) struct SocietyRunnerConfig {
    #[serde(default = "default_executor_binary")]
    pub executor_binary: String,
    #[serde(default = "default_runner_model")]
    pub default_model: String,
    #[serde(default = "default_runner_effort")]
    pub default_effort: String,
    #[serde(default = "default_sleep_seconds")]
    pub default_sleep_seconds: f64,
    #[serde(default = "default_max_sleep_seconds")]
    pub max_sleep_seconds: f64,
    #[serde(default = "default_active_hours_timezone")]
    pub active_hours_timezone: String,
    #[serde(default = "default_active_hours_start")]
    pub active_hours_start: u8,
    #[serde(default = "default_active_hours_end")]
    pub active_hours_end: u8,
    #[serde(default = "default_wall_timeout_seconds")]
    pub wall_timeout_seconds: u64,
    /// Absolute operator cutoff; enforced by both keeper and guest runner.
    #[serde(default)]
    pub hard_stop_at: Option<String>,
    #[serde(default = "default_poll_seconds")]
    pub poll_seconds: f64,
    #[serde(default = "default_quota_threshold")]
    pub quota_threshold_percent: f64,
    #[serde(default = "default_quota_pause_seconds")]
    pub quota_pause_seconds: f64,
    #[serde(default = "default_quota_probe")]
    pub quota_probe: bool,
    #[serde(default = "default_wake_gate_path")]
    pub wake_gate_path: PathBuf,
    #[serde(default = "default_max_active_wakes")]
    pub max_active_wakes: i64,
    #[serde(default = "default_wake_lease_seconds")]
    pub wake_lease_seconds: i64,
}

impl Default for SocietyRunnerConfig {
    fn default() -> Self {
        Self {
            executor_binary: default_executor_binary(),
            default_model: default_runner_model(),
            default_effort: default_runner_effort(),
            default_sleep_seconds: default_sleep_seconds(),
            max_sleep_seconds: default_max_sleep_seconds(),
            active_hours_timezone: default_active_hours_timezone(),
            active_hours_start: default_active_hours_start(),
            active_hours_end: default_active_hours_end(),
            wall_timeout_seconds: default_wall_timeout_seconds(),
            hard_stop_at: None,
            poll_seconds: default_poll_seconds(),
            quota_threshold_percent: default_quota_threshold(),
            quota_pause_seconds: default_quota_pause_seconds(),
            quota_probe: default_quota_probe(),
            wake_gate_path: default_wake_gate_path(),
            max_active_wakes: default_max_active_wakes(),
            wake_lease_seconds: default_wake_lease_seconds(),
        }
    }
}

fn default_world() -> String {
    "honest".into()
}

fn default_society_instance_id() -> String {
    "default".to_string()
}

fn default_society_mission() -> String {
    "You are here. Choose what to do. Stay active.".to_string()
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SocietyModelConfig {
    pub model: String,
    pub input_usd_per_million: f64,
    pub cached_input_usd_per_million: f64,
    pub cache_write_input_usd_per_million: f64,
    pub output_usd_per_million: f64,
}

fn default_treasury_address() -> String {
    "treasury".to_string()
}
fn default_weekly_allowance() -> i64 {
    100
}
fn default_compute_backing() -> SocietyComputeBacking {
    SocietyComputeBacking::Obols
}
fn default_kit_dir() -> PathBuf {
    PathBuf::from("data/kits")
}
fn default_executor_binary() -> String {
    "pi".to_string()
}
fn default_runner_model() -> String {
    "luna".to_string()
}
fn default_runner_effort() -> String {
    "high".to_string()
}
fn default_sleep_seconds() -> f64 {
    300.0
}
fn default_max_sleep_seconds() -> f64 {
    300.0
}
fn default_active_hours_timezone() -> String {
    "Europe/Vienna".to_string()
}
fn default_active_hours_start() -> u8 {
    8
}
fn default_active_hours_end() -> u8 {
    22
}
fn default_wall_timeout_seconds() -> u64 {
    86_400
}
fn default_poll_seconds() -> f64 {
    0.2
}
fn default_quota_threshold() -> f64 {
    85.0
}
fn default_quota_pause_seconds() -> f64 {
    600.0
}
fn default_quota_probe() -> bool {
    true
}
fn default_wake_gate_path() -> PathBuf {
    PathBuf::from("data/wake-gate.sqlite")
}
fn default_max_active_wakes() -> i64 {
    3
}
fn default_wake_lease_seconds() -> i64 {
    180
}

fn default_society_models() -> HashMap<String, SocietyModelConfig> {
    [
        ("luna", "openai-codex/gpt-5.6-luna", 0.2, 0.02, 0.25, 1.2),
        (
            "terra",
            "openai-codex/gpt-5.6-terra",
            2.5,
            0.25,
            3.125,
            15.0,
        ),
        ("sol", "openai-codex/gpt-5.6-sol", 5.0, 0.5, 6.25, 30.0),
    ]
    .into_iter()
    .map(|(alias, model, input, cached, cache_write, output)| {
        (
            alias.to_string(),
            SocietyModelConfig {
                model: model.to_string(),
                input_usd_per_million: input,
                cached_input_usd_per_million: cached,
                cache_write_input_usd_per_million: cache_write,
                output_usd_per_million: output,
            },
        )
    })
    .collect()
}

impl FugueConfig {
    pub(crate) fn load() -> anyhow::Result<Self> {
        let path = env::var_os("FUGUE_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
        let mut config = Self::load_from_path(path)?;
        config.apply_environment_overrides()?;
        config.validate()?;
        Ok(config)
    }

    fn load_from_path(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .with_context(|| format!("failed to read Fugue config {}", path.display()))?;
        let mut config: Self = toml::from_str(&source)
            .with_context(|| format!("failed to parse Fugue config {}", path.display()))?;
        config.resolve_relative_paths(path);
        config.validate()?;
        Ok(config)
    }

    fn apply_environment_overrides(&mut self) -> anyhow::Result<()> {
        if let Some(value) = env::var_os("FUGUE_DATA_DIR") {
            self.storage.data_dir = PathBuf::from(value);
        }
        if let Some(value) = environment_override("FUGUE_API_HOST")? {
            self.api.host = value
                .parse()
                .context("FUGUE_API_HOST must be an IP address")?;
        }
        if let Some(value) = environment_override("FUGUE_API_PORT")? {
            self.apply_api_port_override(parse_port("FUGUE_API_PORT", &value)?)?;
        }
        if let Some(value) = environment_override("FUGUE_UI_HOST")? {
            self.ui.host = value
                .parse()
                .context("FUGUE_UI_HOST must be an IP address")?;
        }
        if let Some(value) = environment_override("FUGUE_UI_PORT")? {
            self.ui.port = parse_port("FUGUE_UI_PORT", &value)?;
        }
        if let Some(value) = environment_override("FUGUE_PREVIEW_PORT")? {
            self.ui.preview_port = parse_port("FUGUE_PREVIEW_PORT", &value)?;
        }
        if let Some(value) = environment_override("FUGUE_KIT_DIR")? {
            self.society.kit_dir = PathBuf::from(value);
        }
        Ok(())
    }

    fn resolve_relative_paths(&mut self, config_path: &Path) {
        let root = config_path.parent().unwrap_or_else(|| Path::new("."));
        if self.society.world != "honest" && Path::new(&self.society.world).is_relative() {
            self.society.world = root
                .join(&self.society.world)
                .to_string_lossy()
                .into_owned();
        }
        if self.storage.data_dir.is_relative() {
            self.storage.data_dir = root.join(&self.storage.data_dir);
        }
        if self.society.kit_dir.is_relative() {
            self.society.kit_dir = root.join(&self.society.kit_dir);
        }
        if self.society.runner.wake_gate_path.is_relative() {
            self.society.runner.wake_gate_path = root.join(&self.society.runner.wake_gate_path);
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.api.host != IpAddr::V4(HOST_API_ADDRESS) {
            anyhow::bail!("api.host must be exactly 127.0.0.1");
        }
        if let Some(guest_api_url) = &self.society.guest_api_url {
            let guest_address = parse_guest_api_url(guest_api_url)?;
            if guest_address.port() != self.api.port {
                anyhow::bail!("society.guest_api_url port must equal api.port");
            }
        }
        if self.llm.model.trim().is_empty() {
            anyhow::bail!("llm.model must not be empty");
        }
        if self.llm.effort.trim().is_empty() {
            anyhow::bail!("llm.effort must not be empty");
        }
        if self.llm.request_timeout_seconds == 0 {
            anyhow::bail!("llm.request_timeout_seconds must be greater than zero");
        }
        if self.sources.forum_magnum.endpoint.trim().is_empty() {
            anyhow::bail!("sources.forum_magnum.endpoint must not be empty");
        }
        if self.society.instance_id.trim().is_empty()
            || !self
                .society
                .instance_id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || self.society.mission.trim().is_empty()
            || self.society.treasury_address.trim().is_empty()
            || self.society.runner.executor_binary.trim().is_empty()
        {
            anyhow::bail!("society identity, mission, treasury, and executor binary must be valid");
        }
        if self.society.weekly_allowance <= 0
            || self.society.runner.default_sleep_seconds < 0.0
            || self.society.runner.max_sleep_seconds < 0.0
            || self.society.runner.default_sleep_seconds > self.society.runner.max_sleep_seconds
            || self.society.runner.active_hours_timezone.trim().is_empty()
            || self.society.runner.active_hours_start >= self.society.runner.active_hours_end
            || self.society.runner.active_hours_end > 24
            || self.society.runner.wall_timeout_seconds == 0
            || self.society.runner.poll_seconds <= 0.0
            || !(0.0..=100.0).contains(&self.society.runner.quota_threshold_percent)
            || self.society.runner.max_active_wakes < 1
            || self.society.runner.wake_lease_seconds < 60
        {
            anyhow::bail!(
                "society identity, mission, allowance, timing, wake gate, and quota values must be valid"
            );
        }
        if let Some(cutoff) = &self.society.runner.hard_stop_at {
            time::OffsetDateTime::parse(cutoff, &time::format_description::well_known::Rfc3339)
                .context("society.runner.hard_stop_at must be an RFC3339 timestamp")?;
        }
        for (alias, model) in &self.society.models {
            if model.model.trim().is_empty()
                || [
                    model.input_usd_per_million,
                    model.cached_input_usd_per_million,
                    model.cache_write_input_usd_per_million,
                    model.output_usd_per_million,
                ]
                .into_iter()
                .any(|rate| !rate.is_finite() || rate < 0.0)
            {
                anyhow::bail!("society model {alias} must have a model id and non-negative rates");
            }
        }
        if !self
            .society
            .models
            .contains_key(&self.society.runner.default_model)
        {
            anyhow::bail!("society.runner.default_model must name a configured society model");
        }
        if self.api.port == self.ui.port
            || self.api.port == self.ui.preview_port
            || self.ui.port == self.ui.preview_port
        {
            anyhow::bail!("api.port, ui.port, and ui.preview_port must be distinct");
        }
        Ok(())
    }

    pub(crate) fn request_timeout(&self) -> Duration {
        Duration::from_secs(self.llm.request_timeout_seconds)
    }

    pub(crate) fn api_listeners(&self) -> anyhow::Result<ApiListeners> {
        let host = SocketAddr::new(self.api.host, self.api.port);
        let guest = if let Ok(port) = env::var("FUGUE_GUEST_BIND_PORT") {
            let port: u16 = port.parse().context("invalid FUGUE_GUEST_BIND_PORT")?;
            if port == 0 || port == self.api.port {
                anyhow::bail!("guest transport requires a separate nonzero port");
            }
            Some(SocketAddr::new(IpAddr::V4(HOST_API_ADDRESS), port))
        } else {
            self.society
                .guest_api_url
                .as_deref()
                .map(parse_guest_api_url)
                .transpose()?
                .map(SocketAddr::V4)
        };
        Ok(ApiListeners {
            host,
            guest: guest.filter(|guest| *guest != host),
        })
    }

    /// `FUGUE_API_PORT` is one shared API-port override. It changes both
    /// listeners instead of weakening the required host/guest port equality.
    fn apply_api_port_override(&mut self, port: u16) -> anyhow::Result<()> {
        let guest = self
            .society
            .guest_api_url
            .as_deref()
            .map(parse_guest_api_url)
            .transpose()?;
        self.api.port = port;
        if let Some(guest) = guest {
            self.society.guest_api_url = Some(format!("http://{}:{port}", guest.ip()));
        }
        Ok(())
    }
}

fn parse_guest_api_url(input: &str) -> anyhow::Result<SocketAddrV4> {
    if input != input.trim() {
        anyhow::bail!("society.guest_api_url must not contain surrounding whitespace");
    }
    let url = reqwest::Url::parse(input).context(
        "society.guest_api_url must be an HTTP URL with a literal IPv4 address and port",
    )?;
    if url.scheme() != "http" {
        anyhow::bail!("society.guest_api_url must use http");
    }
    if url.query().is_some() || url.fragment().is_some() || url.path() != "/" {
        anyhow::bail!("society.guest_api_url must not contain a query, fragment, or non-root path");
    }

    let (_, remainder) = input.split_once("://").ok_or_else(|| {
        anyhow::anyhow!(
            "society.guest_api_url must be an HTTP URL with a literal IPv4 address and port"
        )
    })?;
    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    if !matches!(&remainder[authority_end..], "" | "/") {
        anyhow::bail!("society.guest_api_url must not contain a query, fragment, or non-root path");
    }
    if authority.contains('@') || !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("society.guest_api_url must not contain credentials");
    }
    let address = authority.parse::<SocketAddrV4>().with_context(
        || "society.guest_api_url must contain a literal IPv4 address and explicit non-zero port",
    )?;
    if address.port() == 0 {
        anyhow::bail!("society.guest_api_url port must be non-zero");
    }
    if !is_allowed_guest_api_ip(*address.ip()) {
        anyhow::bail!("society.guest_api_url host must be 127.0.0.1 or an RFC1918 IPv4 address");
    }
    Ok(address)
}

fn is_allowed_guest_api_ip(ip: Ipv4Addr) -> bool {
    ip == HOST_API_ADDRESS || ip.is_private()
}

fn environment_override(name: &str) -> anyhow::Result<Option<String>> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => {
            anyhow::bail!("{name} must not be empty when set")
        }
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn parse_port(name: &str, value: &str) -> anyhow::Result<u16> {
    value
        .parse::<u16>()
        .with_context(|| format!("{name} must be a valid non-zero TCP port"))
        .and_then(|port| {
            if port == 0 {
                anyhow::bail!("{name} must be a valid non-zero TCP port");
            }
            Ok(port)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn config_source() -> &'static str {
        r#"
[storage]
data_dir = "state"

[api]
host = "127.0.0.1"
port = 41001

[ui]
host = "127.0.0.1"
port = 41002
preview_port = 41003

[llm]
model = "openai/gpt-5.6-luna"
effort = "medium"
request_timeout_seconds = 42

[llm.agents.full_generation]
effort = "high"

[sources.forum_magnum]
endpoint = "https://www.lesswrong.com/graphql"

[society]
guest_api_url = "http://127.0.0.1:41001"
"#
    }

    fn parsed_config() -> FugueConfig {
        toml::from_str(config_source()).unwrap()
    }

    #[test]
    fn society_compute_backing_is_typed_and_defaults_to_obols() {
        assert_eq!(
            parsed_config().society.compute_backing,
            SocietyComputeBacking::Obols
        );
        let independent = config_source().replace(
            "guest_api_url = \"http://127.0.0.1:41001\"",
            "guest_api_url = \"http://127.0.0.1:41001\"\ncompute_backing = \"independent\"",
        );
        assert_eq!(
            toml::from_str::<FugueConfig>(&independent)
                .unwrap()
                .society
                .compute_backing,
            SocietyComputeBacking::Independent
        );
        let invalid = independent.replace("independent", "credits");
        assert!(toml::from_str::<FugueConfig>(&invalid).is_err());
    }

    #[test]
    fn loads_typed_config_and_resolves_data_dir_from_config_location() {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("fugue-config-test-{nonce}"));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("custom.toml");
        fs::write(&path, config_source()).unwrap();

        let config = FugueConfig::load_from_path(&path).unwrap();
        assert_eq!(config.storage.data_dir, root.join("state"));
        assert_eq!(config.api.port, 41001);
        assert_eq!(config.ui.preview_port, 41003);
        assert_eq!(
            config.llm.agents["full_generation"].effort.as_deref(),
            Some("high")
        );
        assert_eq!(config.request_timeout(), Duration::from_secs(42));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_unknown_configuration_fields() {
        let source = config_source().replace(
            "data_dir = \"state\"",
            "data_dir = \"state\"\nunexpected = true",
        );
        assert!(toml::from_str::<FugueConfig>(&source).is_err());
    }

    #[test]
    fn host_api_listener_is_exact_loopback() {
        for host in ["0.0.0.0", "127.0.0.2", "192.168.64.1", "::1"] {
            let mut config = parsed_config();
            config.api.host = host.parse().unwrap();
            assert_eq!(
                config.validate().unwrap_err().to_string(),
                "api.host must be exactly 127.0.0.1"
            );
        }
    }

    #[test]
    fn guest_api_listener_accepts_only_exact_loopback_or_rfc1918_ipv4() {
        for host in [
            "127.0.0.1",
            "10.0.0.1",
            "10.255.255.254",
            "172.16.0.1",
            "172.31.255.254",
            "192.168.0.1",
            "192.168.255.254",
        ] {
            assert!(parse_guest_api_url(&format!("http://{host}:41001")).is_ok());
        }

        for host in [
            "0.0.0.0",
            "8.8.8.8",
            "100.64.0.1",
            "100.127.255.254",
            "127.0.0.2",
            "169.254.1.1",
            "224.0.0.1",
            "255.255.255.255",
        ] {
            assert!(parse_guest_api_url(&format!("http://{host}:41001")).is_err());
        }
    }

    #[test]
    fn guest_api_url_rejects_non_listener_url_features() {
        for input in [
            "https://127.0.0.1:41001",
            "http://user:secret@127.0.0.1:41001",
            "http://@127.0.0.1:41001",
            "http://127.0.0.1:41001/api",
            "http://127.0.0.1:41001/./",
            "http://127.0.0.1:41001/api/..",
            "http://127.0.0.1:41001?query=value",
            "http://127.0.0.1:41001#fragment",
            "http://localhost:41001",
            "http://127.1:41001",
            "http://[::1]:41001",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            " http://127.0.0.1:41001",
        ] {
            assert!(parse_guest_api_url(input).is_err(), "accepted {input}");
        }
    }

    #[test]
    fn guest_api_port_must_match_the_host_api_port() {
        let mut config = parsed_config();
        config.society.guest_api_url = Some("http://192.168.64.1:41004".to_string());
        assert_eq!(
            config.validate().unwrap_err().to_string(),
            "society.guest_api_url port must equal api.port"
        );
    }

    #[test]
    fn listener_plan_omits_a_duplicate_loopback_listener() {
        let mut config = parsed_config();
        assert_eq!(
            config.api_listeners().unwrap(),
            ApiListeners {
                host: "127.0.0.1:41001".parse().unwrap(),
                guest: None,
            }
        );

        config.society.guest_api_url = Some("http://192.168.64.1:41001".to_string());
        assert_eq!(
            config.api_listeners().unwrap().guest,
            Some("192.168.64.1:41001".parse().unwrap())
        );
    }

    #[test]
    fn api_port_override_updates_both_listener_ports() {
        let mut config = parsed_config();
        config.society.guest_api_url = Some("http://192.168.64.1:41001".to_string());

        config.apply_api_port_override(42001).unwrap();

        assert_eq!(config.api.port, 42001);
        assert_eq!(
            config.society.guest_api_url.as_deref(),
            Some("http://192.168.64.1:42001")
        );
        assert_eq!(
            config.api_listeners().unwrap().guest,
            Some("192.168.64.1:42001".parse().unwrap())
        );
        config.validate().unwrap();
    }
}
