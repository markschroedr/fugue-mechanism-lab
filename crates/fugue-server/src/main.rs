use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Extension, Json, Router,
    extract::{ConnectInfo, Path, Query, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{Next, from_fn_with_state},
    response::{IntoResponse, Response},
    routing::{get, patch, post, put},
};
use fugue_contract::{AuthInput, HealthResponse};
use fugue_core::{
    OpenAiResponsesClient, ResponseFunctionCall, ResponsesOutput, ResponsesRequest, engine_info,
    validate_model_pricing, validate_reasoning_effort,
};
use fugue_store::{
    AppendMessageInput, ArmExperimentInjectionInput, AuthActor, BirthSocietyAgentInput,
    ChangeClassification, ChangeEntityStateInput, CreateBoardPostInput, CreateBoardSpaceInput,
    CreateEntityInput, CreateEscrowInput, CreateSocietyAgentInput, CreateSpaceInput,
    DeclareConditionInput, DirectoryRegistrationInput, EditEntityTextInput,
    EmitExperimentSignalInput, EntityRole, InputKind, LessWrongAuthorInput, LessWrongMessageInput,
    LessWrongThreadInput, MintObolsInput, NewspaperSections, OpenSessionInput,
    PublicObjectionInput, PutRefreshScheduleInput, RecordSocietyTranscriptInput,
    RecordSocietyUsageInput, SessionReadModel, SleepStartedInput, SocietyActor,
    SocietyAgentCredential, StoreError, StoreInfo, SubmitInput, SynthesisMode, SynthesizeInput,
    ThreadPositionProposal, TransferObolsInput, WakeGate, WakeStartedInput, WorkerJob,
    WriteReportInput, initialize, validate_society_newspaper_sections,
    validate_thread_position_proposals,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tower_http::cors::{Any, CorsLayer};

mod config;
mod context;
mod world;

use config::{FugueConfig, LlmConfig, SocietyConfig};

#[derive(Clone)]
struct AppState {
    store: Arc<StoreInfo>,
    api_host: IpAddr,
    llm: Option<OpenAiResponsesClient>,
    agent_llm_profiles: HashMap<&'static str, AgentLlmProfile>,
    forum_magnum_endpoint: String,
    substrate_key_hash: Option<String>,
    society: SocietyConfig,
    world_run_id: String,
    world: Arc<world::WorldBundle>,
    wake_gate: Arc<WakeGate>,
    objection_rate_limit: Arc<Mutex<HashMap<IpAddr, VecDeque<Instant>>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentLlmProfile {
    model: String,
    effort: String,
}

const SESSION_COOKIE: &str = "fugue_session";
const SESSION_SECONDS: i64 = 60 * 60 * 24 * 30;
const OBJECTION_RATE_LIMIT: usize = 5;
const OBJECTION_RATE_WINDOW: Duration = Duration::from_secs(60);
const GUEST_LISTENER_RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if std::env::args().nth(1).as_deref() == Some("context") {
        println!("{}", context::definitions_json()?);
        return Ok(());
    }
    let config = FugueConfig::load()?;
    let store = initialize(config.storage.data_dir.clone())?;
    store.ensure_society_treasury(&config.society.treasury_address)?;
    if std::env::args().nth(1).as_deref() == Some("init") {
        println!("init=done");
        println!("data_dir={}", store.root.display());
        return Ok(());
    }
    let global_model_override = std::env::var("FUGUE_LLM_MODEL").ok();
    let agent_llm_profiles =
        resolve_agent_llm_profiles(&config.llm, global_model_override.as_deref())?;
    let forum_magnum_endpoint = config.sources.forum_magnum.endpoint.clone();
    let llm = OpenAiResponsesClient::from_env(
        Some(&config.llm.model),
        Some(&config.llm.effort),
        config.request_timeout(),
    )?;
    println!(
        "llm_model={} llm_reasoning_effort={}",
        llm.as_ref()
            .map(OpenAiResponsesClient::model)
            .unwrap_or("disabled"),
        llm.as_ref()
            .map(OpenAiResponsesClient::reasoning_effort)
            .unwrap_or(&config.llm.effort)
    );
    println!(
        "llm_agents={}",
        context::agent_names()
            .map(|name| {
                let profile = &agent_llm_profiles[name];
                format!("{name}={}/{}", profile.model, profile.effort)
            })
            .collect::<Vec<_>>()
            .join(",")
    );
    let world = Arc::new(world::WorldBundle::load(&config.society.world)?);
    let world_run_id = world.record_run(&store, &config.society)?;
    let state = AppState {
        world_run_id,
        world,
        store: Arc::new(store),
        api_host: config.api.host,
        llm,
        agent_llm_profiles,
        forum_magnum_endpoint,
        substrate_key_hash: std::env::var("FUGUE_SUBSTRATE_KEY")
            .ok()
            .filter(|key| !key.is_empty())
            .map(|key| society_key_hash(&key)),
        society: config.society.clone(),
        wake_gate: Arc::new(WakeGate::open(
            &config.society.runner.wake_gate_path,
            config.society.runner.max_active_wakes,
            config.society.runner.wake_lease_seconds,
        )?),
        objection_rate_limit: Arc::new(Mutex::new(HashMap::new())),
    };
    let worker_state = state.clone();

    let protected = Router::new()
        .route("/api/spaces", post(create_space))
        .route("/api/spaces/{space_id}/report", post(write_report))
        .route("/api/spaces/{space_id}/input", post(submit_input))
        .route("/api/spaces/{space_id}/synthesis", post(enqueue_synthesis))
        .route(
            "/api/spaces/{space_id}/refresh-schedule",
            put(put_refresh_schedule),
        )
        .route("/api/spaces/{space_id}/entities", post(propose_entity))
        .route(
            "/api/spaces/{space_id}/conditions",
            post(propose_conditional_stance),
        )
        .route("/api/entities/{entity_id}", patch(edit_entity_text))
        .route("/api/entities/{entity_id}/state", post(change_entity_state))
        .route("/api/review", get(read_review))
        .route("/api/review/{review_id}/accept", post(accept_review_item))
        .route("/api/review/{review_id}/reject", post(reject_review_item))
        .route("/api/sessions", post(open_session))
        .route("/api/sessions/{thread_id}", get(read_session))
        .route(
            "/api/sessions/{thread_id}/messages",
            post(append_user_message),
        )
        .route("/api/sessions/{thread_id}/discard", post(discard_session))
        .route("/api/sessions/{thread_id}/commit", post(commit_session))
        .route_layer(from_fn_with_state(state.clone(), require_auth));

    let substrate_protected = Router::new()
        .route("/api/society/agents", post(create_society_agent))
        .route("/api/society/birth", post(birth_society_agent))
        .route("/api/society/allowance", post(grant_society_allowance))
        .route("/api/society/world-evidence", get(world::evidence))
        .route(
            "/api/society/agents/{address}/wake",
            post(request_society_wake),
        )
        .route("/api/society/transcripts", post(record_society_transcript))
        .route(
            "/api/society/injections",
            get(read_experiment_injections).post(arm_experiment_injection),
        )
        .route("/api/society/signals", post(emit_experiment_signal))
        .route(
            "/api/society/wake-line",
            get(read_society_wake_line).put(put_society_wake_line),
        )
        .route("/api/bank/mint", post(mint_obols))
        .route("/api/bank/usage", post(record_society_usage))
        .route_layer(from_fn_with_state(state.clone(), require_substrate_auth));

    let society_protected = Router::new()
        .route("/api/world/scorer", get(world::scorer))
        .route("/api/world/submissions", post(world::submit))
        .route("/api/world/leaderboard", get(world::leaderboard))
        .route(
            "/api/world/maintenance/{operation}",
            post(world::maintenance),
        )
        .route("/api/society/me", get(read_society_identity))
        .route("/api/society/budget", get(read_society_budget))
        .route("/api/society/wake-context", get(read_society_wake_context))
        .route(
            "/api/society/wakes/permit",
            post(acquire_society_wake_permit),
        )
        .route(
            "/api/society/wakes/permit/heartbeat",
            post(heartbeat_society_wake_permit),
        )
        .route(
            "/api/society/wakes/permit/release",
            post(release_society_wake_permit),
        )
        .route(
            "/api/society/wakes/start",
            post(record_society_wake_started),
        )
        .route(
            "/api/society/wakes/sleep",
            post(record_society_sleep_started),
        )
        .route("/api/society/wakes/status", get(read_society_wake_status))
        .route("/api/society/wakes/debit", post(debit_society_wake))
        .route("/api/society/wakes/usage", post(record_own_society_usage))
        .route(
            "/api/society/wakes/transcript",
            post(record_own_society_transcript),
        )
        .route("/api/directory/register", post(register_directory_entry))
        .route("/api/bank/transfers", post(transfer_obols))
        .route("/api/bank/escrows", post(create_escrow))
        .route(
            "/api/bank/escrows/{escrow_id}/release",
            post(release_escrow),
        )
        .route(
            "/api/bank/escrows/{escrow_id}/execute",
            post(execute_escrow),
        )
        .route("/api/board/spaces", post(create_board_space))
        .route(
            "/api/board/posts",
            post(create_board_post).layer(axum::extract::DefaultBodyLimit::max(
                fugue_store::MAX_BOARD_POST_REQUEST_BYTES,
            )),
        )
        .route("/api/board/inbox", get(read_board_inbox))
        .route("/api/board/spaces/{space_id}/posts", get(read_board_feed))
        .route(
            "/api/board/spaces/{space_id}/newspaper",
            get(read_society_newspaper).post(enqueue_society_newspaper),
        )
        .route("/api/board/posts/{post_id}/read", post(record_board_read))
        .route_layer(from_fn_with_state(state.clone(), require_society_auth));

    let local_admin = Router::new()
        .route("/api/society/admin", get(read_society_admin_overview))
        .route(
            "/api/society/admin/agents/{address}",
            get(read_society_admin_agent_detail),
        )
        .route_layer(from_fn_with_state(
            state.clone(),
            require_local_or_substrate_auth,
        ));

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/auth/register", post(register))
        .route("/api/auth/login", post(login))
        .route("/api/auth/logout", post(logout))
        .route("/api/auth/session", get(read_auth_session))
        .route("/api/spaces", get(read_spaces))
        .route("/api/spaces/{space_id}/doc", get(read_doc))
        .route("/api/spaces/{space_id}/report", get(read_report))
        .route(
            "/api/spaces/{space_id}/report/history",
            get(read_report_history),
        )
        .route(
            "/api/spaces/{space_id}/report/versions/{version}/diff",
            get(read_report_version_diff),
        )
        .route("/api/spaces/{space_id}/feed", get(read_feed))
        .route(
            "/api/spaces/{space_id}/refresh-schedule",
            get(read_refresh_schedule),
        )
        .route("/api/spaces/{space_id}/falsifiers", get(read_falsifiers))
        .route("/api/spaces/{space_id}/sources", get(read_sources))
        .route(
            "/api/spaces/{space_id}/objections",
            post(submit_public_objection),
        )
        .route("/api/costs", get(read_costs))
        .route("/api/directory", get(read_directory))
        .route("/api/bank", get(read_bank))
        .route("/api/bank/ledger", get(read_obol_ledger))
        .route("/api/bank/usage", get(read_society_usage))
        .route("/api/society/transcripts", get(read_society_transcripts))
        .route("/api/board/spaces", get(read_board_spaces))
        .route("/api/artifacts/{artifact_id}", get(read_artifact))
        .merge(protected)
        .merge(substrate_protected)
        .merge(society_protected)
        .merge(local_admin)
        .with_state(state.clone())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        );

    let api_listeners = config.api_listeners()?;
    let host_listener = tokio::net::TcpListener::bind(api_listeners.host)
        .await
        .with_context(|| format!("failed to bind host API listener {}", api_listeners.host))?;
    let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
    let guest_listener_task = api_listeners.guest.map(|address| {
        let guest_app = app.clone();
        tokio::spawn(async move {
            let result = run_guest_listener(address, guest_app, shutdown_receiver).await;
            if let Err(error) = &result {
                eprintln!("guest API listener stopped: {error:#}");
            }
            result
        })
    });
    tokio::spawn(async move {
        run_worker_loop(worker_state).await;
    });
    let allowance_state = state.clone();
    tokio::spawn(async move {
        run_allowance_loop(allowance_state).await;
    });
    let injection_state = state.clone();
    tokio::spawn(async move {
        run_experiment_injection_loop(injection_state).await;
    });

    let signal_sender = shutdown_sender.clone();
    let host_result = axum::serve(
        host_listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        shutdown_signal().await;
        let _ = signal_sender.send(true);
    })
    .await
    .context("host API server failed");

    let _ = shutdown_sender.send(true);
    if let Some(task) = guest_listener_task {
        task.await.context("guest API listener task failed")??;
    }
    host_result?;

    Ok(())
}

async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    let info = engine_info();
    Json(HealthResponse {
        status: info.status.to_string(),
        engine: info.name.to_string(),
        storage: state.store.status.to_string(),
        instance_id: state.society.instance_id.clone(),
        model: state.llm.as_ref().map(|client| client.model().to_string()),
    })
}

async fn read_costs(
    State(state): State<AppState>,
) -> Result<Json<fugue_store::CostSummary>, ApiError> {
    Ok(Json(state.store.read_cost_summary()?))
}

async fn read_society_admin_overview(
    State(state): State<AppState>,
) -> Result<Json<fugue_store::SocietyAdminOverview>, ApiError> {
    Ok(Json(state.store.read_society_admin_overview(
        &state.society.treasury_address,
    )?))
}

async fn read_society_admin_agent_detail(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<fugue_store::SocietyAdminAgentDetail>, ApiError> {
    Ok(Json(state.store.read_society_admin_agent_detail(
        &state.society.treasury_address,
        &address,
    )?))
}

async fn create_society_agent(
    State(state): State<AppState>,
    Json(input): Json<CreateSocietyAgentInput>,
) -> Result<Json<SocietyAgentCredential>, ApiError> {
    let key = random_token();
    let actor = state
        .store
        .create_society_agent(&input.address, &society_key_hash(&key))?;
    Ok(Json(SocietyAgentCredential {
        actor_id: actor.actor_id,
        address: actor.address,
        key,
    }))
}

async fn birth_society_agent(
    State(state): State<AppState>,
    Json(input): Json<BirthSocietyAgentInput>,
) -> Result<Json<SocietyAgentCredential>, ApiError> {
    if input.key.len() != 64 || !input.key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(StoreError::Validation(
            "society birth key must be a 64-character hexadecimal token".to_string(),
        )
        .into());
    }
    let actor = state.store.birth_society_agent(
        &input.address,
        &input.name,
        &society_key_hash(&input.key),
        state.society.weekly_allowance,
    )?;
    Ok(Json(SocietyAgentCredential {
        actor_id: actor.actor_id,
        address: actor.address,
        key: input.key,
    }))
}

async fn grant_society_allowance(
    State(state): State<AppState>,
) -> Result<Json<fugue_store::AllowanceGrantResult>, ApiError> {
    if !state.world.policy.features().allowance_refill {
        return Err(StoreError::Validation("periodic top-up unavailable".into()).into());
    }
    Ok(Json(state.store.grant_weekly_allowances(
        state.society.weekly_allowance,
        &state.society.treasury_address,
    )?))
}

async fn record_society_wake_started(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<WakeStartedInput>,
) -> Result<Json<fugue_store::SocietyWakeStart>, ApiError> {
    if !state.wake_gate.validate(
        &state.society.instance_id,
        &actor.address,
        input.wake_number,
        &input.permit_token,
    )? {
        return Err(AuthError.into());
    }
    Ok(Json(state.store.record_society_wake_started(
        &actor.actor_id,
        input.wake_number,
        state.world.compute_backing(&state.society) == fugue_store::SocietyComputeBacking::Obols,
    )?))
}

async fn record_society_sleep_started(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<SleepStartedInput>,
) -> Result<Json<fugue_store::SocietyAgentLifecycle>, ApiError> {
    Ok(Json(
        state
            .store
            .record_society_sleep_started(&actor.actor_id, input)?,
    ))
}

async fn read_society_wake_status(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<Option<fugue_store::SocietyAgentLifecycle>>, ApiError> {
    Ok(Json(state.store.read_society_lifecycle(&actor.actor_id)?))
}

async fn request_society_wake(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> Result<Json<fugue_store::SocietyAgentLifecycle>, ApiError> {
    Ok(Json(state.store.request_society_wake(&address)?))
}

async fn read_society_identity(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<fugue_store::SocietyIdentity>, ApiError> {
    Ok(Json(state.store.read_society_identity(&actor.actor_id)?))
}

async fn read_society_budget(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<fugue_store::SocietyBudget>, ApiError> {
    Ok(Json(state.store.read_society_budget(
        &actor.actor_id,
        &state.society.treasury_address,
    )?))
}

async fn read_society_wake_context(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<Value>, ApiError> {
    let inbox = state.store.read_board_inbox(&actor.actor_id)?;
    let paid_posts = inbox
        .iter()
        .filter(|post| post.attached_obols > 0)
        .map(|post| fugue_store::SocietyWakePaidPost {
            headline: post.headline.clone(),
            sender: post.author_address.clone(),
            attached_obols: post.attached_obols,
        })
        .collect();
    let mut context = serde_json::to_value(fugue_store::SocietyWakeContext {
        compute_backing: state.world.compute_backing(&state.society),
        budget: state
            .store
            .read_society_budget(&actor.actor_id, &state.society.treasury_address)?,
        shared_daily_line: state
            .store
            .read_society_wake_line()?
            .unwrap_or_else(|| state.society.shared_daily_line.clone()),
        unread_count: i64::try_from(inbox.len()).map_err(anyhow::Error::from)?,
        paid_posts,
    })
    .map_err(anyhow::Error::from)?;
    context["world"] = json!(state.world.policy.id);
    if state.world.policy.features().scorer {
        context["episode"] = state.store.world_status(
            &state.society.instance_id,
            &actor.actor_id,
            &state.world.policy,
        )?;
    }
    Ok(Json(context))
}

async fn arm_experiment_injection(
    State(state): State<AppState>,
    Json(input): Json<ArmExperimentInjectionInput>,
) -> Result<Json<fugue_store::ExperimentInjection>, ApiError> {
    Ok(Json(state.store.arm_experiment_injection(input)?))
}

async fn read_experiment_injections(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::ExperimentInjection>>, ApiError> {
    Ok(Json(state.store.read_experiment_injections()?))
}

async fn emit_experiment_signal(
    State(state): State<AppState>,
    Json(input): Json<EmitExperimentSignalInput>,
) -> Result<Json<Vec<fugue_store::ExperimentInjection>>, ApiError> {
    Ok(Json(state.store.emit_experiment_signal(&input.name)?))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WakePermitInput {
    wake_number: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WakePermitTokenInput {
    wake_number: i64,
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WakePermitOutput {
    token: String,
    lease_expires_at: i64,
}

async fn acquire_society_wake_permit(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<WakePermitInput>,
) -> Result<Json<WakePermitOutput>, ApiError> {
    let permit = state
        .wake_gate
        .acquire(
            &state.society.instance_id,
            &actor.address,
            input.wake_number,
        )?
        .ok_or(WakeCapacityError)?;
    Ok(Json(WakePermitOutput {
        token: permit.token,
        lease_expires_at: permit.lease_expires_at,
    }))
}

async fn heartbeat_society_wake_permit(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<WakePermitTokenInput>,
) -> Result<Json<WakePermitOutput>, ApiError> {
    let permit = state
        .wake_gate
        .heartbeat(
            &state.society.instance_id,
            &actor.address,
            input.wake_number,
            &input.token,
        )?
        .ok_or_else(|| anyhow::anyhow!("wake permit expired or does not match its owner"))?;
    Ok(Json(WakePermitOutput {
        token: permit.token,
        lease_expires_at: permit.lease_expires_at,
    }))
}

async fn release_society_wake_permit(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<WakePermitTokenInput>,
) -> Result<StatusCode, ApiError> {
    state.wake_gate.release(
        &state.society.instance_id,
        &actor.address,
        input.wake_number,
        &input.token,
    )?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PutWakeLineInput {
    text: String,
}

#[derive(Debug, Serialize)]
struct WakeLineOutput {
    text: String,
}

async fn read_society_wake_line(
    State(state): State<AppState>,
) -> Result<Json<WakeLineOutput>, ApiError> {
    Ok(Json(WakeLineOutput {
        text: state
            .store
            .read_society_wake_line()?
            .unwrap_or_else(|| state.society.shared_daily_line.clone()),
    }))
}

async fn put_society_wake_line(
    State(state): State<AppState>,
    Json(input): Json<PutWakeLineInput>,
) -> Result<Json<WakeLineOutput>, ApiError> {
    let text = state.store.put_society_wake_line(&input.text)?;
    Ok(Json(WakeLineOutput { text }))
}

async fn debit_society_wake(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    body: String,
) -> Result<Json<fugue_store::WakeDebitReceipt>, ApiError> {
    if state.world.compute_backing(&state.society)
        == fugue_store::SocietyComputeBacking::Independent
    {
        return Err(StoreError::Validation(
            "wake debit is unavailable when society.compute_backing is independent".to_string(),
        )
        .into());
    }
    if !body.is_empty() {
        return Err(
            StoreError::Validation("wake debit request body must be empty".to_string()).into(),
        );
    }
    Ok(Json(state.store.debit_wake(
        &actor.actor_id,
        &state.society.treasury_address,
    )?))
}

async fn register_directory_entry(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<DirectoryRegistrationInput>,
) -> Result<Json<fugue_store::DirectoryEntry>, ApiError> {
    Ok(Json(
        state
            .store
            .register_directory_entry(&actor.actor_id, input)?,
    ))
}

async fn read_directory(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::DirectoryEntry>>, ApiError> {
    Ok(Json(state.store.read_directory()?))
}

async fn mint_obols(
    State(state): State<AppState>,
    Json(input): Json<MintObolsInput>,
) -> Result<Json<fugue_store::ObolLedgerEntry>, ApiError> {
    Ok(Json(state.store.mint_obols(None, input)?))
}

async fn transfer_obols(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<TransferObolsInput>,
) -> Result<Json<fugue_store::ObolLedgerEntry>, ApiError> {
    Ok(Json(state.store.transfer_obols(&actor.actor_id, input)?))
}

async fn create_escrow(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<CreateEscrowInput>,
) -> Result<Json<fugue_store::EscrowReadModel>, ApiError> {
    Ok(Json(state.store.create_escrow(&actor.actor_id, input)?))
}

async fn release_escrow(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(escrow_id): Path<String>,
) -> Result<Json<fugue_store::EscrowReadModel>, ApiError> {
    Ok(Json(
        state.store.release_escrow(&actor.actor_id, &escrow_id)?,
    ))
}

async fn execute_escrow(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(escrow_id): Path<String>,
) -> Result<Json<fugue_store::EscrowReadModel>, ApiError> {
    Ok(Json(
        state.store.execute_escrow(&actor.actor_id, &escrow_id)?,
    ))
}

async fn read_bank(
    State(state): State<AppState>,
) -> Result<Json<fugue_store::BankSnapshot>, ApiError> {
    Ok(Json(state.store.read_bank_snapshot()?))
}

async fn read_obol_ledger(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::ObolLedgerEntry>>, ApiError> {
    Ok(Json(state.store.read_obol_ledger()?))
}

async fn record_society_usage(
    State(state): State<AppState>,
    Json(input): Json<RecordSocietyUsageInput>,
) -> Result<Json<fugue_store::SocietyUsageRecord>, ApiError> {
    Ok(Json(state.store.record_society_usage(input)?))
}

async fn record_own_society_usage(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<RecordSocietyUsageInput>,
) -> Result<Json<fugue_store::SocietyUsageRecord>, ApiError> {
    if input.agent_address != actor.address {
        return Err(AuthError.into());
    }
    Ok(Json(state.store.record_society_usage(input)?))
}

#[derive(Debug, Deserialize)]
struct SocietyUsageQuery {
    agent: String,
}

async fn read_society_usage(
    State(state): State<AppState>,
    Query(query): Query<SocietyUsageQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<fugue_store::SocietyUsageRecord>>, ApiError> {
    authorize_society_usage_read(&state, &headers, &query.agent)?;
    Ok(Json(state.store.read_society_usage(&query.agent)?))
}

async fn record_society_transcript(
    State(state): State<AppState>,
    Json(input): Json<RecordSocietyTranscriptInput>,
) -> Result<Json<fugue_store::SocietyTranscript>, ApiError> {
    Ok(Json(state.store.record_society_transcript(input)?))
}

async fn record_own_society_transcript(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<RecordSocietyTranscriptInput>,
) -> Result<Json<fugue_store::SocietyTranscript>, ApiError> {
    if input.agent_address != actor.address {
        return Err(AuthError.into());
    }
    Ok(Json(state.store.record_society_transcript(input)?))
}

async fn read_society_transcripts(
    State(state): State<AppState>,
    Query(query): Query<SocietyUsageQuery>,
    headers: HeaderMap,
) -> Result<Json<Vec<fugue_store::SocietyTranscript>>, ApiError> {
    authorize_society_usage_read(&state, &headers, &query.agent)?;
    Ok(Json(state.store.read_society_transcripts(&query.agent)?))
}

async fn create_board_space(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<CreateBoardSpaceInput>,
) -> Result<Json<fugue_store::BoardSpace>, ApiError> {
    Ok(Json(
        state.store.create_board_space(&actor.actor_id, input)?,
    ))
}

async fn read_board_spaces(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::BoardSpace>>, ApiError> {
    Ok(Json(state.store.read_board_spaces()?))
}

async fn create_board_post(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Json(input): Json<CreateBoardPostInput>,
) -> Result<Json<fugue_store::BoardPost>, ApiError> {
    Ok(Json(state.store.create_world_board_post(
        &actor.actor_id,
        input,
        &state.world.policy,
    )?))
}

async fn read_board_feed(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(space_id): Path<String>,
) -> Result<Json<fugue_store::BoardFeed>, ApiError> {
    Ok(Json(
        state.store.read_board_feed(&actor.actor_id, &space_id)?,
    ))
}

async fn read_board_inbox(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
) -> Result<Json<Vec<fugue_store::BoardPost>>, ApiError> {
    Ok(Json(state.store.read_board_inbox(&actor.actor_id)?))
}

async fn read_society_newspaper(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Option<fugue_store::SocietyNewspaper>>, ApiError> {
    Ok(Json(state.store.read_society_newspaper(&space_id)?))
}

async fn enqueue_society_newspaper(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(space_id): Path<String>,
) -> Result<Json<fugue_store::WorkerJobReceipt>, ApiError> {
    Ok(Json(
        state
            .store
            .enqueue_society_newspaper(&actor.actor_id, &space_id)?,
    ))
}

async fn record_board_read(
    State(state): State<AppState>,
    Extension(actor): Extension<SocietyActor>,
    Path(post_id): Path<String>,
) -> Result<Json<fugue_store::BoardReadReceipt>, ApiError> {
    Ok(Json(
        state.store.record_board_read(&actor.actor_id, &post_id)?,
    ))
}

async fn register(
    State(state): State<AppState>,
    Json(input): Json<AuthInput>,
) -> Result<Response, ApiError> {
    validate_auth_input(&input)?;
    if state
        .store
        .credential_for_username(&input.username)?
        .is_some()
    {
        return Err(StoreError::Validation("username is already registered".to_string()).into());
    }
    let salt = SaltString::encode_b64(&rand::random::<[u8; 16]>())
        .map_err(|error| anyhow::anyhow!("password salt generation failed: {error}"))?;
    let password_hash = Argon2::default()
        .hash_password(input.password.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("password hashing failed: {error}"))?
        .to_string();
    let actor = state
        .store
        .register_actor(&input.username, &password_hash)?;
    auth_response(&state, actor)
}

async fn login(
    State(state): State<AppState>,
    Json(input): Json<AuthInput>,
) -> Result<Response, ApiError> {
    validate_auth_input(&input)?;
    let credential = state
        .store
        .credential_for_username(&input.username)?
        .ok_or(AuthError)?;
    let parsed = PasswordHash::new(&credential.password_hash).map_err(|_| AuthError)?;
    Argon2::default()
        .verify_password(input.password.as_bytes(), &parsed)
        .map_err(|_| AuthError)?;
    auth_response(
        &state,
        AuthActor {
            actor_id: credential.actor_id,
            username: credential.username,
        },
    )
}

async fn logout(State(state): State<AppState>, request: Request) -> Result<Response, ApiError> {
    if let Some(token) = session_token(request.headers()) {
        state.store.delete_auth_session(&token)?;
    }
    Ok((
        [(header::SET_COOKIE, expired_cookie())],
        StatusCode::NO_CONTENT,
    )
        .into_response())
}

async fn read_auth_session(
    State(state): State<AppState>,
    request: Request,
) -> Result<Json<Option<AuthActor>>, ApiError> {
    let actor = match session_token(request.headers()) {
        Some(token) => state.store.auth_actor(&token, now_epoch())?,
        None => None,
    };
    Ok(Json(actor))
}

async fn require_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let token = session_token(request.headers()).ok_or(AuthError)?;
    let actor = state
        .store
        .auth_actor(&token, now_epoch())
        .map_err(|_| AuthError)?
        .ok_or(AuthError)?;
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

async fn require_substrate_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let key = bearer_key(request.headers()).ok_or(AuthError)?;
    let expected_hash = state.substrate_key_hash.as_deref().ok_or(AuthError)?;
    if society_key_hash(key) != expected_hash {
        return Err(AuthError);
    }
    Ok(next.run(request).await)
}

#[derive(Clone)]
struct GuestTransport;

async fn require_local_or_substrate_auth(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let is_local = request.extensions().get::<GuestTransport>().is_none()
        && request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .is_some_and(|ConnectInfo(address)| is_local_viewer(address.ip(), state.api_host));
    if is_local {
        return Ok(next.run(request).await);
    }
    require_substrate_auth(State(state), request, next).await
}

fn is_local_viewer(peer: IpAddr, api_host: IpAddr) -> bool {
    peer.is_loopback() || (!api_host.is_unspecified() && peer == api_host)
}

async fn require_society_auth(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let key = bearer_key(request.headers()).ok_or(AuthError)?;
    let actor = state
        .store
        .society_actor_for_key_hash(&society_key_hash(key))
        .map_err(|_| AuthError)?
        .ok_or(AuthError)?;
    let features = state.world.policy.features();
    let path = request.uri().path();
    // Settlement remains possible after termination, preserving final usage and transcripts.
    let settlement = matches!(
        path,
        "/api/society/wakes/usage"
            | "/api/society/wakes/transcript"
            | "/api/society/wakes/debit"
            | "/api/society/wakes/sleep"
            | "/api/society/wakes/permit/release"
            | "/api/society/wakes/status"
            | "/api/society/wake-context"
    );
    if features.scorer
        && !settlement
        && state
            .store
            .world_ended(
                &state.society.instance_id,
                &actor.actor_id,
                state.world.policy.episodes(),
            )
            .map_err(|_| AuthError)?
    {
        return Err(AuthError);
    }
    if !features.obols && path.starts_with("/api/bank/") {
        return Err(AuthError);
    }
    if !features.newspaper && path.ends_with("/newspaper") {
        return Ok((
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"Newspaper unavailable"})),
        )
            .into_response());
    }
    request.extensions_mut().insert(actor);
    Ok(next.run(request).await)
}

fn authorize_society_usage_read(
    state: &AppState,
    headers: &HeaderMap,
    agent_address: &str,
) -> Result<(), AuthError> {
    let key = bearer_key(headers).ok_or(AuthError)?;
    let key_hash = society_key_hash(key);
    if state.substrate_key_hash.as_deref() == Some(key_hash.as_str()) {
        return Ok(());
    }
    let actor = state
        .store
        .society_actor_for_key_hash(&key_hash)
        .map_err(|_| AuthError)?
        .ok_or(AuthError)?;
    if actor.address != agent_address.trim().to_ascii_lowercase() {
        return Err(AuthError);
    }
    Ok(())
}

fn auth_response(state: &AppState, actor: AuthActor) -> Result<Response, ApiError> {
    let token = random_token();
    state
        .store
        .create_auth_session(&token, &actor.actor_id, now_epoch() + SESSION_SECONDS)?;
    Ok(([(header::SET_COOKIE, session_cookie(&token))], Json(actor)).into_response())
}

fn validate_auth_input(input: &AuthInput) -> Result<(), ApiError> {
    let username = input.username.trim();
    if username.len() < 2
        || username.len() > 40
        || !username
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    {
        return Err(StoreError::Validation(
            "username must be 2-40 letters, numbers, _ or -".to_string(),
        )
        .into());
    }
    if input.password.len() < 8 {
        return Err(
            StoreError::Validation("password must be at least 8 characters".to_string()).into(),
        );
    }
    Ok(())
}

fn random_token() -> String {
    rand::random::<[u8; 32]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn bearer_key(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .filter(|key| !key.is_empty())
}

fn society_key_hash(key: &str) -> String {
    format!("{:x}", Sha256::digest(key.as_bytes()))
}

fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

fn session_token(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|cookie| {
            let (name, value) = cookie.trim().split_once('=')?;
            (name == SESSION_COOKIE && !value.is_empty()).then(|| value.to_string())
        })
}

fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_SECONDS}")
}

fn expired_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

async fn read_spaces(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::SpaceRef>>, ApiError> {
    Ok(Json(state.store.read_spaces()?))
}

async fn read_doc(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<fugue_store::SynthesisDoc>, ApiError> {
    Ok(Json(state.store.read_doc(&space_id)?))
}

async fn create_space(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Json(input): Json<CreateSpaceInput>,
) -> Result<Json<fugue_store::SpaceRef>, ApiError> {
    Ok(Json(state.store.create_space(&actor.actor_id, input)?))
}

async fn read_report(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Option<fugue_store::SynthesisReport>>, ApiError> {
    Ok(Json(state.store.read_report(&space_id)?))
}

async fn write_report(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(space_id): Path<String>,
    Json(input): Json<WriteReportInput>,
) -> Result<Json<fugue_store::SynthesisReport>, ApiError> {
    Ok(Json(state.store.write_authored_report(
        &actor.actor_id,
        &space_id,
        input,
    )?))
}

async fn read_report_history(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Vec<fugue_store::ReportVersion>>, ApiError> {
    Ok(Json(state.store.read_report_history(&space_id)?))
}

async fn read_report_version_diff(
    State(state): State<AppState>,
    Path((space_id, version)): Path<(String, i64)>,
) -> Result<Json<fugue_store::ReportVersionDiff>, ApiError> {
    Ok(Json(
        state.store.read_report_version_diff(&space_id, version)?,
    ))
}

#[derive(Debug, Deserialize)]
struct FeedQuery {
    after: Option<String>,
}

async fn read_feed(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
    Query(query): Query<FeedQuery>,
) -> Result<Json<Vec<fugue_store::FeedEntry>>, ApiError> {
    Ok(Json(
        state.store.read_feed(&space_id, query.after.as_deref())?,
    ))
}

async fn read_refresh_schedule(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Option<fugue_store::RefreshSchedule>>, ApiError> {
    Ok(Json(state.store.read_refresh_schedule(&space_id)?))
}

async fn put_refresh_schedule(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
    Json(input): Json<PutRefreshScheduleInput>,
) -> Result<Json<fugue_store::RefreshSchedule>, ApiError> {
    Ok(Json(state.store.put_refresh_schedule(&space_id, input)?))
}

async fn read_falsifiers(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Vec<fugue_store::FalsifierGroup>>, ApiError> {
    Ok(Json(state.store.read_falsifiers(&space_id)?))
}

async fn propose_conditional_stance(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(space_id): Path<String>,
    Json(input): Json<DeclareConditionInput>,
) -> Result<Json<fugue_store::ReviewItem>, ApiError> {
    Ok(Json(state.store.propose_conditional_stance(
        &actor.actor_id,
        &space_id,
        input,
    )?))
}

async fn submit_input(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(space_id): Path<String>,
    Json(input): Json<SubmitInput>,
) -> Result<Json<fugue_store::InputReceipt>, ApiError> {
    if input.kind != InputKind::Comment {
        if let Some(post_id) = lesswrong_post_id(&input.content) {
            return Ok(Json(state.store.enqueue_lesswrong_thread_fetch(
                &actor.actor_id,
                &space_id,
                &post_id,
            )?));
        }
    }
    Ok(Json(state.store.submit_input(
        &actor.actor_id,
        &space_id,
        input,
    )?))
}

async fn submit_public_objection(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(space_id): Path<String>,
    Json(input): Json<PublicObjectionInput>,
) -> Result<Json<fugue_store::ReviewItem>, ApiError> {
    enforce_objection_rate_limit(&state, peer.ip())?;
    Ok(Json(state.store.submit_public_objection(&space_id, input)?))
}

fn enforce_objection_rate_limit(state: &AppState, ip: IpAddr) -> Result<(), ApiError> {
    let now = Instant::now();
    let mut limits = state
        .objection_rate_limit
        .lock()
        .map_err(|_| anyhow::anyhow!("objection rate limiter unavailable"))?;
    limits.retain(|_, attempts| {
        while attempts
            .front()
            .is_some_and(|attempt| now.duration_since(*attempt) >= OBJECTION_RATE_WINDOW)
        {
            attempts.pop_front();
        }
        !attempts.is_empty()
    });
    let attempts = limits.entry(ip).or_default();
    if attempts.len() >= OBJECTION_RATE_LIMIT {
        return Err(RateLimitError.into());
    }
    attempts.push_back(now);
    Ok(())
}

async fn read_sources(
    State(state): State<AppState>,
    Path(space_id): Path<String>,
) -> Result<Json<Vec<fugue_store::ImportedSource>>, ApiError> {
    Ok(Json(state.store.read_imported_sources(&space_id)?))
}

async fn read_artifact(
    State(state): State<AppState>,
    Path(artifact_id): Path<String>,
) -> Result<Json<fugue_store::ArtifactReadModel>, ApiError> {
    Ok(Json(state.store.read_artifact(&artifact_id)?))
}

async fn enqueue_synthesis(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(space_id): Path<String>,
    Json(input): Json<SynthesizeInput>,
) -> Result<Json<fugue_store::WorkerJobReceipt>, ApiError> {
    Ok(Json(state.store.enqueue_synthesis(
        &actor.actor_id,
        &space_id,
        input,
    )?))
}

async fn propose_entity(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(space_id): Path<String>,
    Json(input): Json<CreateEntityInput>,
) -> Result<Json<fugue_store::ReviewItem>, ApiError> {
    Ok(Json(state.store.propose_entity(
        &actor.actor_id,
        &space_id,
        input,
    )?))
}

async fn edit_entity_text(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(entity_id): Path<String>,
    Json(input): Json<EditEntityTextInput>,
) -> Result<Json<fugue_store::SynthesisDoc>, ApiError> {
    Ok(Json(state.store.edit_entity_text(&actor.actor_id, &entity_id, input)?))
}

async fn change_entity_state(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(entity_id): Path<String>,
    Json(input): Json<ChangeEntityStateInput>,
) -> Result<Json<fugue_store::SynthesisDoc>, ApiError> {
    Ok(Json(state.store.change_entity_state(&actor.actor_id, &entity_id, input)?))
}

async fn read_review(
    State(state): State<AppState>,
) -> Result<Json<Vec<fugue_store::ReviewItem>>, ApiError> {
    Ok(Json(state.store.read_review()?))
}

async fn open_session(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Json(input): Json<OpenSessionInput>,
) -> Result<Json<SessionReadModel>, ApiError> {
    Ok(Json(state.store.open_session(&actor.actor_id, input)?))
}

async fn read_session(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(thread_id): Path<String>,
) -> Result<Json<SessionReadModel>, ApiError> {
    Ok(Json(state.store.read_session(&actor.actor_id, &thread_id)?))
}

async fn append_user_message(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(thread_id): Path<String>,
    Json(input): Json<AppendMessageInput>,
) -> Result<Json<SessionReadModel>, ApiError> {
    Ok(Json(state.store.append_user_message(
        &actor.actor_id,
        &thread_id,
        input,
    )?))
}

async fn discard_session(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(thread_id): Path<String>,
) -> Result<Json<SessionReadModel>, ApiError> {
    Ok(Json(
        state.store.discard_session(&actor.actor_id, &thread_id)?,
    ))
}

async fn commit_session(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(thread_id): Path<String>,
) -> Result<Json<fugue_store::ReviewItem>, ApiError> {
    Ok(Json(
        state.store.commit_session(&actor.actor_id, &thread_id)?,
    ))
}

async fn reject_review_item(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(review_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .reject_review_item(&actor.actor_id, &review_id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn accept_review_item(
    State(state): State<AppState>,
    Extension(actor): Extension<AuthActor>,
    Path(review_id): Path<String>,
) -> Result<Json<fugue_store::SynthesisDoc>, ApiError> {
    Ok(Json(state.store.accept_review_item(&actor.actor_id, &review_id)?))
}

#[derive(Debug)]
struct RateLimitError;

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("anonymous objection rate limit exceeded; try again later")
    }
}

impl std::error::Error for RateLimitError {}

#[derive(Debug)]
struct WakeCapacityError;

impl std::fmt::Display for WakeCapacityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("all host wake slots are active; retry later")
    }
}

impl std::error::Error for WakeCapacityError {}

#[derive(Debug)]
struct AuthError;

impl std::fmt::Display for AuthError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("authentication required")
    }
}

impl std::error::Error for AuthError {}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

#[derive(Debug)]
struct ApiError(anyhow::Error);

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(error: E) -> Self {
        Self(error.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = if self.0.downcast_ref::<AuthError>().is_some() {
            StatusCode::UNAUTHORIZED
        } else if self.0.downcast_ref::<RateLimitError>().is_some()
            || self.0.downcast_ref::<WakeCapacityError>().is_some()
        {
            StatusCode::TOO_MANY_REQUESTS
        } else {
            match self.0.downcast_ref::<StoreError>() {
                Some(StoreError::Validation(_)) => StatusCode::BAD_REQUEST,
                Some(StoreError::NotFound(_)) => StatusCode::NOT_FOUND,
                None => StatusCode::INTERNAL_SERVER_ERROR,
            }
        };
        (
            status,
            Json(ErrorResponse {
                error: self.0.to_string(),
            }),
        )
            .into_response()
    }
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

async fn run_guest_listener(
    address: SocketAddr,
    app: Router,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let mut reported_wait = false;
    let listener = loop {
        match tokio::net::TcpListener::bind(address).await {
            Ok(listener) => break listener,
            Err(error) => {
                if !reported_wait {
                    eprintln!("guest API listener waiting for {address}: {error}");
                    reported_wait = true;
                }
                tokio::select! {
                    _ = tokio::time::sleep(GUEST_LISTENER_RETRY_INTERVAL) => {}
                    _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
                }
            }
        }
    };
    println!("guest_api_listener={address}");
    axum::serve(
        listener,
        app.layer(Extension(GuestTransport))
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        wait_for_shutdown(&mut shutdown).await;
    })
    .await
    .context("guest API server failed")
}

async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
}

fn lesswrong_post_id(input: &str) -> Option<String> {
    let url = reqwest::Url::parse(input.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !matches!(url.host_str(), Some("lesswrong.com" | "www.lesswrong.com"))
    {
        return None;
    }
    let segments = url.path_segments()?.collect::<Vec<_>>();
    if segments.first().copied() != Some("posts") {
        return None;
    }
    let id = segments.get(1)?.trim();
    (!id.is_empty()
        && id
            .chars()
            .all(|character| character.is_ascii_alphanumeric()))
    .then(|| id.to_string())
}

#[derive(Debug, Deserialize)]
struct LessWrongGraphResponse {
    data: Option<LessWrongGraphData>,
    errors: Option<Vec<LessWrongGraphError>>,
}

#[derive(Debug, Deserialize)]
struct LessWrongGraphError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct LessWrongGraphData {
    post: LessWrongPostResult,
    comments: LessWrongCommentsResult,
}

#[derive(Debug, Deserialize)]
struct LessWrongPostResult {
    result: Option<LessWrongPost>,
}

#[derive(Debug, Deserialize)]
struct LessWrongCommentsResult {
    results: Vec<LessWrongComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LessWrongPost {
    #[serde(rename = "_id")]
    id: String,
    title: String,
    slug: String,
    posted_at: String,
    contents: LessWrongContents,
    user: Option<LessWrongUser>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LessWrongComment {
    #[serde(rename = "_id")]
    id: String,
    parent_comment_id: Option<String>,
    posted_at: String,
    contents: LessWrongContents,
    user: Option<LessWrongUser>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LessWrongContents {
    plaintext_main_text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LessWrongUser {
    #[serde(rename = "_id")]
    id: String,
    #[serde(rename = "displayName")]
    display_name: String,
    slug: Option<String>,
}

async fn fetch_lesswrong_thread(
    endpoint: &str,
    post_id: &str,
) -> anyhow::Result<LessWrongThreadInput> {
    const QUERY: &str = r#"
      query FugueLessWrongThread($postId: String!, $offset: Int!, $limit: Int!) {
        post(input: {selector: {_id: $postId}}) {
          result { _id title slug postedAt contents { plaintextMainText } user { _id displayName slug } }
        }
        comments(input: {terms: {postId: $postId, view: "postCommentsTop", limit: $limit, offset: $offset}}) {
          results { _id parentCommentId postedAt contents { plaintextMainText } user { _id displayName slug } }
        }
      }
    "#;
    const PAGE_SIZE: usize = 200;
    let client = reqwest::Client::new();
    let mut offset = 0usize;
    let mut post = None;
    let mut graph_comments = Vec::new();
    loop {
        let response = client
            .post(endpoint)
            .json(&json!({
                "query": QUERY,
                "variables": { "postId": post_id, "offset": offset, "limit": PAGE_SIZE }
            }))
            .send()
            .await?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(
                "LessWrong GraphQL returned {status}: {body}"
            ));
        }
        let payload: LessWrongGraphResponse = serde_json::from_str(&body)?;
        if let Some(errors) = payload.errors.filter(|errors| !errors.is_empty()) {
            return Err(anyhow::anyhow!(
                "LessWrong GraphQL error: {}",
                errors
                    .into_iter()
                    .map(|error| error.message)
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        let data = payload
            .data
            .ok_or_else(|| anyhow::anyhow!("LessWrong GraphQL response had no data"))?;
        if post.is_none() {
            post = data.post.result;
        }
        let page_len = data.comments.results.len();
        graph_comments.extend(data.comments.results);
        if page_len < PAGE_SIZE {
            break;
        }
        offset += page_len;
    }
    let post = post.ok_or_else(|| StoreError::NotFound("LessWrong post not found".to_string()))?;
    let post_author = graph_author(post.user, &format!("post-{}", post.id));
    let url = format!("https://www.lesswrong.com/posts/{}/{}", post.id, post.slug);
    let comments = graph_comments
        .into_iter()
        .map(|comment| {
            let content = comment
                .contents
                .plaintext_main_text
                .unwrap_or_default()
                .trim()
                .to_string();
            let fallback = format!("comment-{}", comment.id);
            LessWrongMessageInput {
                external_id: comment.id,
                parent_external_id: comment.parent_comment_id,
                author: graph_author(comment.user, &fallback),
                content,
                posted_at: comment.posted_at,
            }
        })
        .collect();
    Ok(LessWrongThreadInput {
        post_id: post.id,
        title: post.title,
        slug: post.slug,
        url,
        author: post_author,
        content: post.contents.plaintext_main_text.unwrap_or_default(),
        posted_at: post.posted_at,
        comments,
    })
}

fn graph_author(user: Option<LessWrongUser>, fallback: &str) -> LessWrongAuthorInput {
    match user {
        Some(user) => LessWrongAuthorInput {
            external_id: user.id,
            display_name: user.display_name,
            slug: user.slug,
        },
        None => LessWrongAuthorInput {
            external_id: fallback.to_string(),
            display_name: "Deleted user".to_string(),
            slug: None,
        },
    }
}

fn resolve_agent_llm_profiles(
    config: &LlmConfig,
    global_model_override: Option<&str>,
) -> anyhow::Result<HashMap<&'static str, AgentLlmProfile>> {
    validate_model_pricing(&config.model)?;
    validate_reasoning_effort(&config.effort)?;
    for (name, profile) in &config.agents {
        if !context::agent_names().any(|declared| declared == name) {
            return Err(anyhow::anyhow!(
                "LLM configuration names undeclared agent {name}"
            ));
        }
        if let Some(model) = &profile.model {
            validate_model_pricing(model)?;
        }
        if let Some(effort) = &profile.effort {
            validate_reasoning_effort(effort)?;
        }
    }
    if let Some(model) = global_model_override {
        validate_model_pricing(model)?;
    }

    let default_model = global_model_override.unwrap_or(config.model.as_str());
    let default_effort = config.effort.as_str();
    context::agent_names()
        .map(|name| {
            let profile = config.agents.get(name);
            let model = global_model_override
                .or_else(|| profile.and_then(|profile| profile.model.as_deref()))
                .unwrap_or(default_model);
            let effort = profile
                .and_then(|profile| profile.effort.as_deref())
                .unwrap_or(default_effort);
            validate_model_pricing(model)?;
            validate_reasoning_effort(effort)?;
            Ok((
                name,
                AgentLlmProfile {
                    model: model.to_string(),
                    effort: effort.to_string(),
                },
            ))
        })
        .collect()
}

async fn run_worker_loop(state: AppState) {
    match state.store.requeue_running_worker_jobs() {
        Ok(count) if count > 0 => {
            eprintln!("worker startup recovery requeued {count} running job(s)");
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("worker startup recovery failed: {error:#}");
        }
    }

    loop {
        if let Err(error) = state.store.scan_due_refresh_schedules() {
            eprintln!("refresh schedule scan failed: {error:#}");
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        match state.store.claim_next_worker_job() {
            Ok(Some(job)) => run_worker_job(&state, job).await,
            Ok(None) => tokio::time::sleep(Duration::from_millis(250)).await,
            Err(error) => {
                eprintln!("worker claim failed: {error:#}");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

async fn run_allowance_loop(state: AppState) {
    if !state.world.policy.features().allowance_refill {
        return;
    }
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await;
        if let Err(error) = state.store.grant_weekly_allowances(
            state.society.weekly_allowance,
            &state.society.treasury_address,
        ) {
            eprintln!("weekly society allowance scan failed: {error:#}");
        }
    }
}

async fn run_experiment_injection_loop(state: AppState) {
    loop {
        if let Err(error) = state.store.fire_due_experiment_injections() {
            eprintln!("experiment injection scan failed: {error:#}");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn run_worker_job(state: &AppState, job: WorkerJob) {
    let attempt = match state.store.start_worker_attempt(&job.id) {
        Ok(attempt) => attempt,
        Err(error) => {
            let message = format!("failed to start worker attempt: {error:#}");
            if let Err(error) = state.store.mark_worker_job_failed(&job.id, &message) {
                eprintln!("worker job failure record failed: {error:#}");
            }
            return;
        }
    };

    let result = execute_worker_job(state, &job, &attempt.id).await;
    match result {
        Ok(summary) => {
            if let Err(error) = state.store.finish_worker_attempt(&attempt.id, &summary) {
                eprintln!("worker attempt finish failed: {error:#}");
            }
            if let Err(error) = state.store.mark_worker_job_succeeded(&job.id) {
                eprintln!("worker job finish failed: {error:#}");
            }
        }
        Err(error) => {
            let message = error.to_string();
            if let Err(error) = state.store.fail_worker_attempt(&attempt.id, &message) {
                eprintln!("worker attempt failure record failed: {error:#}");
            }
            if let Err(error) = state.store.mark_worker_job_failed(&job.id, &message) {
                eprintln!("worker job failure record failed: {error:#}");
            }
        }
    }
}

async fn tracked_respond(
    state: &AppState,
    attempt_id: &str,
    stage: &str,
    agent_name: &'static str,
    request: ResponsesRequest,
) -> anyhow::Result<ResponsesOutput> {
    let llm = state
        .llm
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("configured LLM API key is not set"))?;
    let profile = state
        .agent_llm_profiles
        .get(agent_name)
        .ok_or_else(|| anyhow::anyhow!("missing resolved LLM profile for {agent_name}"))?;
    let result = llm
        .respond_with_profile(request, &profile.model, &profile.effort)
        .await?;
    let mut usage = serde_json::to_value(&result.usage)?;
    usage["stage"] = Value::String(stage.to_string());
    usage["agent"] = Value::String(agent_name.to_string());
    usage["requestedEffort"] = Value::String(profile.effort.clone());
    state
        .store
        .record_worker_attempt_llm_call(attempt_id, &usage)?;
    Ok(result.output)
}

const VALIDATION_ATTEMPTS: usize = 2;

fn should_retry_validation(attempt_index: usize) -> bool {
    attempt_index + 1 < VALIDATION_ATTEMPTS
}

async fn validated_respond<T, F>(
    state: &AppState,
    attempt_id: &str,
    stage: &str,
    request: context::AgentRequest,
    mut validate: F,
) -> anyhow::Result<T>
where
    F: FnMut(ResponsesOutput) -> anyhow::Result<T>,
{
    let agent_name = request.agent_name;
    let request = request.request;
    let mut feedback = None;
    for attempt_index in 0..VALIDATION_ATTEMPTS {
        let request = match feedback.as_deref() {
            Some(error) => context::with_validation_feedback(request.clone(), error)?,
            None => request.clone(),
        };
        let output = tracked_respond(state, attempt_id, stage, agent_name, request).await?;
        let validated = match output {
            ResponsesOutput::Invalid(message) => {
                Err(anyhow::anyhow!("invalid Responses output: {message}"))
            }
            output => validate(output),
        };
        match validated {
            Ok(value) => return Ok(value),
            Err(error) if should_retry_validation(attempt_index) => {
                eprintln!(
                    "llm_validation_retry agent={agent_name} attempt={} error={error:#}",
                    attempt_index + 2
                );
                feedback = Some(error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("validation attempt count is nonzero")
}

async fn execute_worker_job(
    state: &AppState,
    job: &WorkerJob,
    attempt_id: &str,
) -> anyhow::Result<String> {
    match job.kind.as_str() {
        "agent_reply" => {
            let payload: Value = serde_json::from_str(&job.payload_json)?;
            let thread_id = payload
                .get("thread_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("agent_reply job missing thread_id"))?;
            let through_message_id = payload
                .get("through_message_id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("agent_reply job missing through_message_id"))?;
            let request = context::session_reply(&state.store, thread_id, through_message_id)?;
            let text =
                validated_respond(state, attempt_id, "other", request, |output| match output {
                    ResponsesOutput::Text(text) if !text.trim().is_empty() => Ok(text),
                    ResponsesOutput::Text(_) => {
                        Err(anyhow::anyhow!("agent reply returned empty text"))
                    }
                    _ => Err(anyhow::anyhow!("agent reply did not return text")),
                })
                .await?;
            state.store.append_agent_message(thread_id, text.trim())?;
            Ok("agent reply appended".to_string())
        }
        "ingest" => execute_ingest_job(state, job, attempt_id).await,
        "lesswrong_thread_fetch" => execute_lesswrong_thread_fetch_job(state, job).await,
        "lesswrong_thread_ingest" => {
            execute_lesswrong_thread_ingest_job(state, job, attempt_id).await
        }
        "society_newspaper" => execute_society_newspaper_job(state, job, attempt_id).await,
        "synthesize" => execute_synthesis_job(state, job, attempt_id).await,
        other => Err(anyhow::anyhow!("unsupported worker job kind: {other}")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SocietyNewspaperJobPayload {
    space_id: String,
}

async fn execute_society_newspaper_job(
    state: &AppState,
    job: &WorkerJob,
    attempt_id: &str,
) -> anyhow::Result<String> {
    if !state.world.policy.features().newspaper {
        return Ok("Newspaper disabled by world".into());
    }
    if let Some(newspaper) = state.store.committed_society_newspaper_effect(&job.id)? {
        return Ok(format!(
            "Society Newspaper v{} already committed; recovered without replay",
            newspaper.version
        ));
    }
    let payload: SocietyNewspaperJobPayload = serde_json::from_str(&job.payload_json)?;
    let newspaper_context = state
        .store
        .build_society_newspaper_context(&payload.space_id)?;
    if newspaper_context.new_public_posts.is_empty() {
        return Ok("Society Newspaper no-op: no new public posts".to_string());
    }
    let request = context::society_newspaper(&newspaper_context)?;
    let sections: NewspaperSections =
        validated_respond(state, attempt_id, "society_newspaper", request, |output| {
            let sections = match output {
                ResponsesOutput::Json(value) => serde_json::from_value(value)?,
                _ => {
                    return Err(anyhow::anyhow!(
                        "Society Newspaper did not return structured output"
                    ));
                }
            };
            validate_society_newspaper_sections(&newspaper_context, &sections)?;
            Ok(sections)
        })
        .await?;
    let newspaper = state
        .store
        .write_society_newspaper(&job.id, &newspaper_context, sections)?;
    Ok(format!("Society Newspaper v{} written", newspaper.version))
}

async fn execute_lesswrong_thread_fetch_job(
    state: &AppState,
    job: &WorkerJob,
) -> anyhow::Result<String> {
    let payload: Value = serde_json::from_str(&job.payload_json)?;
    let post_id = payload
        .get("postId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("LessWrong fetch job missing postId"))?;
    let space_id = payload
        .get("spaceId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("LessWrong fetch job missing spaceId"))?;
    let thread = fetch_lesswrong_thread(&state.forum_magnum_endpoint, post_id).await?;
    let receipt = state.store.import_lesswrong_thread(space_id, thread)?;
    Ok(match receipt.ingest_job_id {
        Some(job_id) => format!(
            "LessWrong refresh found {} new source artifact(s); merge job {job_id} queued",
            receipt.new_source_count,
        ),
        None => "LessWrong refresh complete: no new material".to_string(),
    })
}

#[derive(Debug, Deserialize)]
struct IngestProposal {
    role: EntityRole,
    text: String,
}

async fn execute_ingest_job(
    state: &AppState,
    job: &WorkerJob,
    attempt_id: &str,
) -> anyhow::Result<String> {
    let payload: Value = serde_json::from_str(&job.payload_json)?;
    let artifact_id = payload
        .get("artifactId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("ingest job missing artifactId"))?;
    let request = context::coarse_ingest(&state.store, artifact_id)?;
    let proposal: IngestProposal = validated_respond(
        state,
        attempt_id,
        "ingest_extraction",
        request,
        |output| match output {
            ResponsesOutput::Json(value) => {
                let proposal: IngestProposal = serde_json::from_value(value)?;
                if proposal.text.trim().is_empty() {
                    return Err(anyhow::anyhow!("ingest proposal text is required"));
                }
                Ok(proposal)
            }
            _ => Err(anyhow::anyhow!("ingest did not return structured output")),
        },
    )
    .await?;
    let review = state.store.record_ingest_proposal(
        artifact_id,
        CreateEntityInput {
            role: proposal.role,
            text: proposal.text,
        },
    )?;
    Ok(format!("ingest proposal queued for review {}", review.id))
}

async fn execute_lesswrong_thread_ingest_job(
    state: &AppState,
    job: &WorkerJob,
    attempt_id: &str,
) -> anyhow::Result<String> {
    let payload: Value = serde_json::from_str(&job.payload_json)?;
    let artifact_id = payload
        .get("artifactId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("LessWrong ingest job missing artifactId"))?;
    let space_id = payload
        .get("spaceId")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("LessWrong ingest job missing spaceId"))?;
    let new_source_artifact_ids = payload
        .get("newSourceArtifactIds")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("LessWrong ingest job missing newSourceArtifactIds"))?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect::<Vec<_>>();
    let (request, merge_context) = context::lesswrong_landscape(
        &state.store,
        artifact_id,
        space_id,
        &new_source_artifact_ids,
    )?;
    let positions: Vec<ThreadPositionProposal> =
        validated_respond(state, attempt_id, "ingest_extraction", request, |output| {
            let value = match output {
                ResponsesOutput::Json(value) => value,
                _ => {
                    return Err(anyhow::anyhow!(
                        "LessWrong ingest did not return structured output"
                    ));
                }
            };
            let positions: Vec<ThreadPositionProposal> = serde_json::from_value(
                value
                    .get("positions")
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("landscape missing positions"))?,
            )?;
            validate_thread_position_proposals(&merge_context, &positions)?;
            Ok(positions)
        })
        .await?;
    let count = positions.len();
    let review = state
        .store
        .record_thread_ingest_proposal(merge_context, positions)?;
    Ok(match review {
        Some(review) => format!(
            "{count} landscape delta finding(s) queued for review {}",
            review.id
        ),
        None => "new discourse produced no landscape delta".to_string(),
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SynthesisJobPayload {
    space_id: String,
    mode: SynthesisMode,
    instruction: Option<String>,
    #[serde(default)]
    auto: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QualityAssessment {
    classifications: Vec<InputClassification>,
    depth: SynthesisMode,
    routing_reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputClassification {
    event_id: String,
    classification: ChangeClassification,
    reason: String,
    visible_reaction: String,
}

fn validate_quality_assessment(assessment: &QualityAssessment) -> anyhow::Result<()> {
    if assessment.routing_reason.trim().is_empty() {
        return Err(anyhow::anyhow!(
            "synthesis quality assessment routing reason is required"
        ));
    }
    if assessment.classifications.iter().any(|item| {
        item.event_id.trim().is_empty()
            || item.reason.trim().is_empty()
            || item.visible_reaction.trim().is_empty()
    }) {
        return Err(anyhow::anyhow!(
            "each synthesis classification requires an event, reason, and visible reaction"
        ));
    }
    Ok(())
}

async fn execute_synthesis_job(
    state: &AppState,
    job: &WorkerJob,
    attempt_id: &str,
) -> anyhow::Result<String> {
    if let Some(report) = state.store.committed_synthesis_effect(&job.id)? {
        return Ok(format!(
            "synthesis report v{} already committed; recovered without replay",
            report.version
        ));
    }
    let payload: SynthesisJobPayload = serde_json::from_str(&job.payload_json)?;
    let mut context = state.store.build_synthesis_context(&payload.space_id)?;
    if payload.auto && context.causes.is_empty() {
        return Ok("automatic synthesis no-op: no accepted inputs".to_string());
    }
    let assessment = if context.causes.is_empty() {
        None
    } else {
        let request = context::classifier(&context)?;
        let (assessment, classified_causes) =
            validated_respond(state, attempt_id, "classification", request, |output| {
                let value = match output {
                    ResponsesOutput::Json(value) => value,
                    _ => {
                        return Err(anyhow::anyhow!(
                            "synthesis quality assessment did not return structured output"
                        ));
                    }
                };
                let assessment: QualityAssessment = serde_json::from_value(value)?;
                validate_quality_assessment(&assessment)?;
                let mut classified_causes = context.causes.clone();
                attach_classifications(&mut classified_causes, &assessment.classifications)?;
                Ok((assessment, classified_causes))
            })
            .await?;
        context.causes = classified_causes;
        Some(assessment)
    };
    let (mode, routing_reason) = if payload.auto {
        let decision = assessment.as_ref().ok_or_else(|| {
            anyhow::anyhow!("automatic synthesis had no accepted inputs to assess")
        })?;
        state.store.record_synthesis_routing(
            &job.id,
            decision.depth.clone(),
            &decision.routing_reason,
        )?;
        (
            decision.depth.clone(),
            Some(decision.routing_reason.clone()),
        )
    } else {
        (payload.mode, None)
    };
    let all_redundant = !context.causes.is_empty()
        && context
            .causes
            .iter()
            .all(|cause| cause.change_classification == Some(ChangeClassification::Redundant));

    let redundant_content = if all_redundant {
        context
            .current_report
            .as_ref()
            .map(|report| report.content.clone())
    } else {
        None
    };
    let content = if let Some(content) = redundant_content {
        content
    } else {
        match mode {
            SynthesisMode::Full => {
                let request = context::full_generation(&context)?;
                validated_respond(state, attempt_id, "synthesis", request, |output| {
                    let content = match output {
                        ResponsesOutput::Text(text) => text,
                        _ => {
                            return Err(anyhow::anyhow!(
                                "full synthesis did not return report text"
                            ));
                        }
                    };
                    let content = ensure_native_attribution(content, &context.causes);
                    let content = ensure_classified_cruxes(content, assessment.as_ref());
                    ensure_quality_structure(&content)?;
                    Ok(content)
                })
                .await?
            }
            SynthesisMode::Selective => {
                let current_report = context.current_report.as_ref().ok_or_else(|| {
                    anyhow::anyhow!("selective synthesis requires a current report")
                })?;
                let instruction = payload
                    .instruction
                    .as_deref()
                    .or(routing_reason.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("selective synthesis missing instruction"))?;
                let request = context::selective_edit(&context, instruction)?;
                validated_respond(state, attempt_id, "synthesis", request, |output| {
                    let calls = match output {
                        ResponsesOutput::FunctionCalls(calls) => calls,
                        _ => {
                            return Err(anyhow::anyhow!(
                                "selective synthesis did not call an edit tool"
                            ));
                        }
                    };
                    let content = apply_text_edits(&current_report.content, &calls)?;
                    let content = ensure_native_attribution(content, &context.causes);
                    let content = ensure_classified_cruxes(content, assessment.as_ref());
                    ensure_quality_structure(&content)?;
                    Ok(content)
                })
                .await?
            }
        }
    };
    ensure_quality_structure(&content)?;
    let report = state.store.write_synthesis_report(
        Some(&job.id),
        &payload.space_id,
        mode,
        routing_reason.as_deref(),
        &content,
        &context.causes,
    )?;
    if std::env::var_os("FUGUE_TEST_CRASH_AFTER_SYNTHESIS_COMMIT").is_some() {
        eprintln!(
            "test crash hook: synthesis report v{} committed for job {}",
            report.version, job.id
        );
        std::process::exit(86);
    }
    Ok(format!("synthesis report v{} written", report.version))
}

fn attach_classifications(
    causes: &mut [fugue_store::ReportCause],
    classifications: &[InputClassification],
) -> anyhow::Result<()> {
    if causes.len() != classifications.len() {
        return Err(anyhow::anyhow!(
            "quality assessment returned {} classifications for {} causes",
            classifications.len(),
            causes.len()
        ));
    }
    for cause in causes {
        let mut matches = classifications
            .iter()
            .filter(|item| item.event_id == cause.event_id);
        let classification = matches.next().ok_or_else(|| {
            anyhow::anyhow!("quality assessment omitted event {}", cause.event_id)
        })?;
        if matches.next().is_some() {
            return Err(anyhow::anyhow!(
                "quality assessment duplicated event {}",
                cause.event_id
            ));
        }
        cause.change_classification = Some(classification.classification.clone());
        cause.classification_reason = Some(classification.reason.clone());
    }
    Ok(())
}

fn ensure_native_attribution(mut content: String, causes: &[fugue_store::ReportCause]) -> String {
    let Some(source_heading) = content.find("## Source record") else {
        return content;
    };
    let mut insertion = String::new();
    for cause in causes {
        if cause.change_classification == Some(ChangeClassification::Redundant) {
            continue;
        }
        let (Some(author), Some(claim)) = (&cause.author_name, &cause.attributed_claim) else {
            continue;
        };
        let claim = claim.split_whitespace().collect::<Vec<_>>().join(" ");
        insertion.push_str(&format!("\n\n- {author} argues: {claim}"));
    }
    if !insertion.is_empty() {
        content.insert_str(source_heading + "## Source record".len(), &insertion);
        content.insert(
            source_heading + "## Source record".len() + insertion.len(),
            '\n',
        );
    }
    content
}

fn ensure_classified_cruxes(mut content: String, assessment: Option<&QualityAssessment>) -> String {
    let Some(assessment) = assessment else {
        return content;
    };
    for input in &assessment.classifications {
        if input.classification == ChangeClassification::NewTension
            && !content.contains(&input.visible_reaction)
        {
            let reaction = input
                .visible_reaction
                .trim()
                .strip_prefix("Add a neutral crux stating that ")
                .or_else(|| {
                    input
                        .visible_reaction
                        .trim()
                        .strip_prefix("A neutral crux should state that ")
                })
                .unwrap_or(input.visible_reaction.trim());
            content.push_str("\n\n- ");
            content.push_str(reaction);
            content.push('\n');
        }
    }
    content
}

fn ensure_quality_structure(content: &str) -> anyhow::Result<()> {
    let required = [
        "## Source record",
        "## Synthesis interpretation",
        "## Cruxes",
    ];
    let mut title_seen = false;
    let mut section_seen = false;
    let mut fence: Option<(u8, usize)> = None;
    let mut headings = Vec::new();

    for line in content.lines() {
        if let Some((marker, minimum_length)) = fence {
            if closing_fence(line, marker, minimum_length) {
                fence = None;
            }
            continue;
        }

        if !section_seen {
            if line.trim().is_empty() {
                continue;
            }
            if !title_seen
                && line
                    .strip_prefix("# ")
                    .is_some_and(|title| !title.trim().is_empty())
            {
                title_seen = true;
                continue;
            }
            if line != required[0] {
                return Err(anyhow::anyhow!(
                    "synthesis quality contract found unexpected content before {}: {:?}",
                    if title_seen {
                        "## Source record"
                    } else {
                        "the optional title or ## Source record"
                    },
                    line
                ));
            }
            section_seen = true;
        }

        if let Some(marker) = opening_fence(line) {
            fence = Some(marker);
        } else if line.starts_with("# ") {
            return Err(anyhow::anyhow!(
                "synthesis quality contract found an unexpected title after the report began: {:?}",
                line
            ));
        } else if line.starts_with("## ") {
            headings.push(line);
        }
    }

    if fence.is_some() {
        return Err(anyhow::anyhow!(
            "synthesis quality contract found an unclosed fenced code block"
        ));
    }
    if headings != required {
        return Err(anyhow::anyhow!(
            "synthesis quality contract found sections [{}]; requires exactly the ordered sections: {}",
            headings.join(", "),
            required.join(", ")
        ));
    }
    Ok(())
}

fn opening_fence(line: &str) -> Option<(u8, usize)> {
    let trimmed = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let marker = *trimmed.as_bytes().first()?;
    if !matches!(marker, b'`' | b'~') {
        return None;
    }
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    if length < 3 || (marker == b'`' && trimmed[length..].contains('`')) {
        return None;
    }
    Some((marker, length))
}

fn closing_fence(line: &str, marker: u8, minimum_length: usize) -> bool {
    let trimmed = line
        .strip_prefix("   ")
        .or_else(|| line.strip_prefix("  "))
        .or_else(|| line.strip_prefix(' '))
        .unwrap_or(line);
    let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
    length >= minimum_length && trimmed[length..].trim().is_empty()
}

fn apply_text_edits(report: &str, calls: &[ResponseFunctionCall]) -> anyhow::Result<String> {
    let mut edited = report.to_string();
    let mut replaced_bytes = 0usize;
    for call in calls {
        match call.name.as_str() {
            "replace_exact" => {
                let old = required_argument(&call.arguments, "old")?;
                let new = required_argument_allow_empty(&call.arguments, "new")?;
                if old == report {
                    return Err(anyhow::anyhow!(
                        "selective edit cannot replace the complete report"
                    ));
                }
                replaced_bytes = replaced_bytes.saturating_add(old.len());
                let index = unique_match_index(&edited, old)?;
                edited.replace_range(index..index + old.len(), new);
            }
            "insert_after_anchor" => {
                let anchor = required_argument(&call.arguments, "anchor")?;
                let text = required_argument(&call.arguments, "text")?;
                let start = unique_match_index(&edited, anchor)?;
                let index = start + anchor.len();
                let starts_on_boundary = start == 0 || edited.as_bytes()[start - 1] == b'\n';
                let ends_on_boundary = index == edited.len() || edited.as_bytes()[index] == b'\n';
                if !starts_on_boundary || !ends_on_boundary {
                    return Err(anyhow::anyhow!(
                        "insert anchor must be a complete markdown block or heading"
                    ));
                }
                let trailing = &edited[index..];
                let insertion = if trailing.starts_with("\n\n") {
                    format!("\n\n{}", text.trim())
                } else {
                    format!("\n\n{}\n", text.trim())
                };
                edited.insert_str(index, &insertion);
            }
            other => return Err(anyhow::anyhow!("unsupported text-edit tool: {other}")),
        }
    }
    if replaced_bytes.saturating_mul(2) > report.len() {
        return Err(anyhow::anyhow!(
            "selective edit would replace more than half of the report"
        ));
    }
    // A model can legally choose an identity replacement. Preserve the audited
    // report version rather than failing the worker after classification.
    Ok(edited)
}

fn required_argument<'a>(arguments: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    let value = required_argument_allow_empty(arguments, name)?;
    if value.is_empty() {
        return Err(anyhow::anyhow!("tool argument {name} must not be empty"));
    }
    Ok(value)
}

fn required_argument_allow_empty<'a>(arguments: &'a Value, name: &str) -> anyhow::Result<&'a str> {
    arguments
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("tool call missing string argument {name}"))
}

fn unique_match_index(haystack: &str, needle: &str) -> anyhow::Result<usize> {
    let mut matches = haystack.match_indices(needle);
    let (index, _) = matches
        .next()
        .ok_or_else(|| anyhow::anyhow!("edit anchor was not found exactly"))?;
    if matches.next().is_some() {
        return Err(anyhow::anyhow!("edit anchor is not unique"));
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn guest_listener_retries_until_it_can_serve_with_connect_info() {
        async fn peer_ip(ConnectInfo(peer): ConnectInfo<SocketAddr>) -> String {
            peer.ip().to_string()
        }

        let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = blocker.local_addr().unwrap();
        let app = Router::new().route("/peer", get(peer_ip));
        let (shutdown_sender, shutdown_receiver) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(run_guest_listener(address, app, shutdown_receiver));

        tokio::time::sleep(Duration::from_millis(20)).await;
        drop(blocker);

        let response = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match reqwest::get(format!("http://{address}/peer")).await {
                    Ok(response) if response.status().is_success() => {
                        break response.text().await.unwrap();
                    }
                    _ => tokio::time::sleep(Duration::from_millis(20)).await,
                }
            }
        })
        .await
        .expect("guest listener did not bind after the address became available");
        assert_eq!(response, "127.0.0.1");

        shutdown_sender.send(true).unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("guest listener did not shut down")
            .unwrap()
            .unwrap();
    }

    #[test]
    fn local_viewer_accepts_only_loopback_or_the_exact_api_host() {
        let bridge_host = "192.168.64.1".parse().unwrap();
        assert!(is_local_viewer("127.0.0.1".parse().unwrap(), bridge_host));
        assert!(is_local_viewer(bridge_host, bridge_host));
        assert!(!is_local_viewer(
            "192.168.64.2".parse().unwrap(),
            bridge_host
        ));
        assert!(!is_local_viewer(
            "10.0.0.5".parse().unwrap(),
            "0.0.0.0".parse().unwrap()
        ));
    }

    #[derive(Deserialize)]
    struct TestRootConfig {
        llm: LlmConfig,
    }

    fn parse_llm_config(source: &str) -> LlmConfig {
        toml::from_str::<TestRootConfig>(source).unwrap().llm
    }

    #[test]
    fn agent_llm_profiles_inherit_and_override_model_and_effort_independently() {
        let config = parse_llm_config(
            r#"
            [llm]
            model = "openai/gpt-5.6-luna"
            effort = "medium"
            request_timeout_seconds = 600

            [llm.agents.lesswrong_landscape]
            model = "x-ai/grok-4.5"

            [llm.agents.full_generation]
            effort = "high"
            "#,
        );
        let profiles = resolve_agent_llm_profiles(&config, None).unwrap();
        assert_eq!(
            profiles["lesswrong_landscape"],
            AgentLlmProfile {
                model: "x-ai/grok-4.5".to_string(),
                effort: "medium".to_string(),
            }
        );
        assert_eq!(
            profiles["full_generation"],
            AgentLlmProfile {
                model: "openai/gpt-5.6-luna".to_string(),
                effort: "high".to_string(),
            }
        );
        assert_eq!(
            profiles["session_reply"],
            AgentLlmProfile {
                model: "openai/gpt-5.6-luna".to_string(),
                effort: "medium".to_string(),
            }
        );
    }

    #[test]
    fn global_model_environment_override_applies_to_every_agent() {
        let config = parse_llm_config(
            r#"
            [llm]
            model = "openai/gpt-5.6-luna"
            effort = "medium"
            request_timeout_seconds = 600
            [llm.agents.lesswrong_landscape]
            model = "x-ai/grok-4.5"
            "#,
        );
        let profiles =
            resolve_agent_llm_profiles(&config, Some("deepseek/deepseek-v4-pro")).unwrap();
        assert!(
            profiles
                .values()
                .all(|profile| profile.model == "deepseek/deepseek-v4-pro")
        );
    }

    #[test]
    fn agent_llm_configuration_rejects_unknown_agents_models_and_efforts() {
        for body in [
            r#"[llm]
               model = "openai/gpt-5.6-luna"
               effort = "medium"
               request_timeout_seconds = 600
               [llm.agents.ghost_writer]
               effort = "high""#,
            r#"[llm]
               model = "openai/gpt-5.6-luna"
               effort = "medium"
               request_timeout_seconds = 600
               [llm.agents.full_generation]
               model = "unpriced/model""#,
            r#"[llm]
               model = "openai/gpt-5.6-luna"
               effort = "medium"
               request_timeout_seconds = 600
               [llm.agents.full_generation]
               effort = "extreme""#,
        ] {
            let config = parse_llm_config(body);
            assert!(resolve_agent_llm_profiles(&config, None).is_err());
        }
    }

    #[test]
    fn output_validation_gets_exactly_one_retry() {
        assert!(should_retry_validation(0));
        assert!(!should_retry_validation(1));
        assert!(!should_retry_validation(2));
    }

    #[test]
    fn lesswrong_urls_are_routed_by_address_only() {
        assert_eq!(
            lesswrong_post_id("https://www.lesswrong.com/posts/z9tXCGogEgkgHSh8G/a-title"),
            Some("z9tXCGogEgkgHSh8G".to_string())
        );
        assert_eq!(
            lesswrong_post_id("https://example.com/posts/abc/title"),
            None
        );
        assert_eq!(lesswrong_post_id("A LessWrong post argues something"), None);
    }

    #[test]
    fn selective_tools_preserve_every_unedited_byte() {
        let report = "Alpha paragraph.\n\nBeta paragraph.";
        let calls = vec![
            ResponseFunctionCall {
                name: "replace_exact".to_string(),
                arguments: json!({ "old": "Alpha", "new": "Revised alpha" }),
            },
            ResponseFunctionCall {
                name: "insert_after_anchor".to_string(),
                arguments: json!({ "anchor": "Beta paragraph.", "text": " Added." }),
            },
        ];

        assert_eq!(
            apply_text_edits(report, &calls).expect("valid edits should apply"),
            "Revised alpha paragraph.\n\nBeta paragraph.\n\nAdded.\n"
        );
    }

    #[test]
    fn selective_tools_reject_a_full_report_replacement() {
        let report = "Alpha paragraph.\n\nBeta paragraph.";
        let calls = vec![ResponseFunctionCall {
            name: "replace_exact".to_string(),
            arguments: json!({ "old": report, "new": "Entirely rewritten report." }),
        }];
        let error = apply_text_edits(report, &calls)
            .expect_err("selective integration must not replace the complete report");
        assert!(error.to_string().contains("complete report"));
    }

    #[test]
    fn quality_structure_requires_distinct_ordered_sections() {
        let report = "# State\n\n## Source record\nRecord.\n\n## Synthesis interpretation\nInference.\n\n## Cruxes\nCrux.";
        ensure_quality_structure(report).expect("quality structure should pass");
        ensure_quality_structure(
            "## Source record\nRecord.\n\n## Synthesis interpretation\nInference.\n\n## Cruxes\nCrux.",
        )
        .expect("the title is optional");
        assert!(ensure_quality_structure("## Source record\nOnly record.").is_err());
        assert!(
            ensure_quality_structure(
                "## Cruxes\nC\n## Source record\nR\n## Synthesis interpretation\nI"
            )
            .is_err()
        );
    }

    #[test]
    fn malformed_backtick_fence_cannot_hide_an_extra_section() {
        let report = "## Source record\n\n```bad`info\n## Extra rendered section\n```\n\n## Synthesis interpretation\nI\n\n## Cruxes\nC";
        ensure_quality_structure(report)
            .expect_err("an invalid fence opener must not hide a rendered heading");
    }

    #[test]
    fn quality_structure_rejects_reasoning_preamble_before_title() {
        let report = "I need to reason through the entities first.\nInternal entity entity-123 supports the claim.\n\n# State\n\n## Source record\nR\n\n## Synthesis interpretation\nI\n\n## Cruxes\nC";
        let error = ensure_quality_structure(report)
            .expect_err("reasoning prose before the title must not be published");
        assert!(error.to_string().contains("I need to reason"));
    }

    #[test]
    fn quality_structure_rejects_preamble_when_title_is_absent() {
        let report = "Here is the report.\n\n## Source record\nR\n\n## Synthesis interpretation\nI\n\n## Cruxes\nC";
        let error = ensure_quality_structure(report)
            .expect_err("preamble before Source record must not be published");
        assert!(error.to_string().contains("Here is the report"));
    }

    #[test]
    fn quality_structure_ignores_section_like_lines_inside_fences() {
        let report = "# State\n\n## Source record\n\n```markdown\n## Synthesis interpretation\n## Cruxes\n```\n\n## Synthesis interpretation\nI\n\n## Cruxes\nC";
        ensure_quality_structure(report).expect("fenced headings are source content");
    }

    #[test]
    fn quality_assessment_must_cover_each_cause_exactly_once() {
        let mut causes = vec![fugue_store::ReportCause {
            event_id: "event-1".to_string(),
            kind: "ingest_entity_proposed".to_string(),
            label: "Accepted input".to_string(),
            source: None,
            author_name: Some("writer".to_string()),
            unverified_display_name: None,
            objection_anchor: None,
            state_change: None,
            input_text: Some("claim".to_string()),
            attributed_claim: Some("compressed claim".to_string()),
            change_classification: None,
            classification_reason: None,
        }];
        let classification = InputClassification {
            event_id: "event-1".to_string(),
            classification: ChangeClassification::NewTension,
            reason: "contradicts the existing position".to_string(),
            visible_reaction: "Atlas and Kepler disagree about scaling.".to_string(),
        };
        attach_classifications(&mut causes, &[classification]).expect("exact coverage should pass");
        assert_eq!(
            causes[0].change_classification,
            Some(ChangeClassification::NewTension)
        );
        let persisted = serde_json::to_value(&causes[0]).expect("cause should serialize");
        assert!(persisted.get("inputText").is_none());
        assert_eq!(persisted["authorName"], "writer");
        assert_eq!(persisted["changeClassification"], "new-tension");
        assert!(attach_classifications(&mut causes, &[]).is_err());
    }

    #[test]
    fn classified_new_tension_is_materialized_as_a_reader_facing_crux() {
        let assessment = QualityAssessment {
            classifications: vec![InputClassification {
                event_id: "event-1".to_string(),
                classification: ChangeClassification::NewTension,
                reason: "conflict".to_string(),
                visible_reaction: "Add a neutral crux stating that Atlas and Kepler share the goal but disagree about scaling.".to_string(),
            }],
            depth: SynthesisMode::Selective,
            routing_reason: "targeted crux".to_string(),
        };
        let report = ensure_classified_cruxes("## Cruxes\n".to_string(), Some(&assessment));
        assert!(report.contains("- Atlas and Kepler share the goal but disagree about scaling."));
        assert!(!report.contains("Add a neutral crux"));
    }

    #[test]
    fn selective_tools_reject_ambiguous_anchors() {
        let calls = vec![ResponseFunctionCall {
            name: "replace_exact".to_string(),
            arguments: json!({ "old": "repeat", "new": "changed" }),
        }];
        let error = apply_text_edits("repeat and repeat", &calls)
            .expect_err("ambiguous anchor should fail instead of guessing");
        assert!(error.to_string().contains("not unique"));
    }

    #[test]
    fn selective_insertions_preserve_markdown_block_boundaries() {
        let report = "## Source record\n\nRecord.\n\n## Synthesis interpretation\n\nInference.\n\n## Cruxes\n\nCrux.";
        let calls = vec![ResponseFunctionCall {
            name: "insert_after_anchor".to_string(),
            arguments: json!({ "anchor": "## Source record", "text": "Writer adds evidence." }),
        }];
        let edited = apply_text_edits(report, &calls).expect("block insertion should apply");
        assert!(edited.contains("## Source record\n\nWriter adds evidence.\n\nRecord."));
        ensure_quality_structure(&edited).expect("insertion must preserve report structure");
    }

    #[test]
    fn selective_identity_edit_degrades_to_audited_no_change() {
        let report = "Alpha paragraph.\n\nBeta paragraph.";
        let calls = vec![ResponseFunctionCall {
            name: "replace_exact".to_string(),
            arguments: json!({ "old": "Alpha", "new": "Alpha" }),
        }];
        assert_eq!(apply_text_edits(report, &calls).unwrap(), report);
    }
}
