//! Trusted operator world bundles. Only the honest world ships with public source.
use super::*;
use fugue_store::{SocietyWorld, WorldSubmissionInput};

pub struct WorldBundle {
    pub policy: SocietyWorld,
    pub definition: Value,
    spawn: String,
    skill: String,
    renderer: Option<String>,
    scorer: Option<String>,
    support: Option<String>,
}

impl WorldBundle {
    pub fn load(selector: &str) -> anyhow::Result<Self> {
        let honest = selector == "honest";
        let directory = std::path::Path::new(selector);
        let definition: Value = serde_json::from_str(&if honest {
            include_str!("../../../resources/society/worlds/honest.json").to_string()
        } else {
            std::fs::read_to_string(directory.join("world.json"))?
        })?;
        let policy: SocietyWorld = serde_json::from_value(definition.clone())?;
        anyhow::ensure!(
            !policy.id.is_empty()
                && policy.reward_obols >= 0
                && policy.reward_obols <= 1_000_000
                && (!policy.features.scorer || policy.episodes_per_agent > 0)
                && (policy.features.obols
                    || (!policy.force_obol_backing
                        && !policy.features.paid_messages
                        && !policy.features.allowance_refill
                        && policy.reward_obols == 0)),
            "invalid world policy"
        );
        let read = |field: &str| -> anyhow::Result<Option<String>> {
            let Some(name) = definition[field].as_str() else {
                return Ok(None);
            };
            anyhow::ensure!(
                !name.is_empty()
                    && std::path::Path::new(name).components().count() == 1
                    && !name.starts_with('.'),
                "world resources must use simple filenames"
            );
            let path = directory.join(name);
            anyhow::ensure!(
                !path.symlink_metadata()?.file_type().is_symlink(),
                "world resources cannot be symlinks"
            );
            Ok(Some(std::fs::read_to_string(path)?))
        };
        let spawn = if honest {
            include_str!("../../../resources/society/SPAWN.md").into()
        } else {
            std::fs::read_to_string(directory.join("SPAWN.md"))?
        };
        let skill = if honest {
            include_str!("../../../resources/society/SKILL-society-node.md").into()
        } else {
            std::fs::read_to_string(directory.join("SKILL-society-node.md"))?
        };
        let renderer = read("wakeRenderer")?;
        let scorer = read("scorer")?;
        let support = read("supportFile")?;
        anyhow::ensure!(
            !policy.features.scorer || scorer.is_some(),
            "world scorer source is required"
        );
        Ok(Self {
            policy,
            definition,
            spawn,
            skill,
            renderer,
            scorer,
            support,
        })
    }
    pub fn compute_backing(&self, society: &SocietyConfig) -> fugue_store::SocietyComputeBacking {
        if !self.policy.features.obols {
            fugue_store::SocietyComputeBacking::Independent
        } else if self.policy.force_obol_backing {
            fugue_store::SocietyComputeBacking::Obols
        } else {
            society.compute_backing
        }
    }
    pub fn record_run(&self, store: &StoreInfo, society: &SocietyConfig) -> anyhow::Result<String> {
        let manifest = json!({
            "world":self.definition,
            "spawnTemplate":self.spawn,
            "skill":self.skill,
            "runnerSource":include_str!("../../../scripts/robinson_runner.py"),
            "wakeRenderer":self.renderer,
            "scorer":self.scorer,
            "support":self.support,
            "mission":society.mission,
            "initialObols":society.weekly_allowance,
            "configuredComputeBacking":society.compute_backing,
            "effectiveComputeBacking":self.compute_backing(society),
            "models":society.models,
            "runner": {
                "defaultModel":society.runner.default_model,
                "defaultEffort":society.runner.default_effort,
                "defaultSleepSeconds":society.runner.default_sleep_seconds,
                "maxSleepSeconds":society.runner.max_sleep_seconds,
                "hardStopAt":society.runner.hard_stop_at,
            },
            "instrumentationDeclarations":society.instrumentation,
        });
        store.record_world_run(&society.instance_id, &manifest)
    }
}

fn require_scorer(state: &AppState) -> Result<(), ApiError> {
    if !state.world.policy.features.scorer {
        return Err(StoreError::NotFound("scorer unavailable".into()).into());
    }
    Ok(())
}

pub async fn scorer(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    require_scorer(&state)?;
    Ok(Json(state.world.definition["agentInterface"].clone()))
}

pub async fn leaderboard(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<Value>, ApiError> {
    require_scorer(&state)?;
    Ok(Json(state.store.world_status(
        &state.society.instance_id,
        &actor.actor_id,
        &state.world.policy,
    )?))
}

pub async fn submit(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<WorldSubmissionInput>,
) -> Result<Json<Value>, ApiError> {
    require_scorer(&state)?;
    input.validate()?;
    let scored_input = serde_json::to_string(&input).map_err(anyhow::Error::from)?;
    let bundle = state.world.clone();
    // Execute operator-owned code, never submitted code. Source is frozen at API startup.
    let scored = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let output = std::process::Command::new("python3")
            .arg("-c").arg("import sys,json; ns={'__name__':'world_scorer'}; exec(sys.argv[1],ns); exec(sys.argv[2],ns); print(json.dumps(ns['score'](json.loads(sys.argv[3]))))")
            .arg(bundle.support.as_deref().unwrap_or("")).arg(bundle.scorer.as_deref().unwrap_or("")).arg(scored_input).output()?;
        anyhow::ensure!(output.status.success(),"scorer failed: {}",String::from_utf8_lossy(&output.stderr));
        Ok(serde_json::from_slice(&output.stdout)?)
    }).await.map_err(anyhow::Error::from)??;
    Ok(Json(state.store.record_world_submission(
        &state.society.instance_id,
        &state.world_run_id,
        &actor.actor_id,
        &state.world.policy,
        &input,
        &scored,
    )?))
}

pub async fn maintenance(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(operation): Path<String>,
) -> Result<Json<Value>, ApiError> {
    require_scorer(&state)?;
    state.store.terminate_world_agent(
        &state.society.instance_id,
        &state.world_run_id,
        &actor.actor_id,
        &operation,
        &state.world.policy,
    )?;
    Ok(Json(json!({})))
}

pub async fn evidence(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    let mut evidence = state.store.world_evidence()?;
    evidence["activeRunId"] = json!(state.world_run_id);
    Ok(Json(evidence))
}
