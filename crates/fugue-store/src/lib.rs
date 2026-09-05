use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::Engine;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use specta::Type;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

mod newspaper;
mod wake_gate;

pub use newspaper::{
    NewspaperEntry, NewspaperPostBody, NewspaperPostSource, NewspaperSections, SocietyNewspaper,
    SocietyNewspaperContext, validate_society_newspaper_sections,
};
use newspaper::{enqueue_newspaper_job, read_society_newspaper_model};
pub use wake_gate::{WakeGate, WakePermitRecord};

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);
const LOCAL_ACTOR_ID: &str = "local-user";
const AGENT_ACTOR_ID: &str = "fugue-agent";
pub const THREAD_MERGE_BATCH_SIZE: usize = 201;
const MAX_SAFE_OBOLS: i64 = 9_007_199_254_740_991;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StoreInfo {
    pub status: &'static str,
    pub root: PathBuf,
    pub core_path: PathBuf,
    pub events_path: PathBuf,
    pub operations_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorePaths {
    pub root: PathBuf,
    pub core: PathBuf,
    pub events: PathBuf,
    pub operations: PathBuf,
}

#[derive(Debug, Clone, Copy)]
struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SpaceRef {
    pub id: String,
    pub title: String,
    pub projection: Projection,
    pub neighbors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum Projection {
    Belief,
    Desire,
    Action,
}

impl Projection {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Belief => "belief",
            Self::Desire => "desire",
            Self::Action => "action",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateSpaceInput {
    pub title: String,
    pub projection: Projection,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct AuthActor {
    pub actor_id: String,
    pub username: String,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct CreateSocietyAgentInput {
    pub address: String,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct BirthSocietyAgentInput {
    pub address: String,
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAgentCredential {
    pub actor_id: String,
    pub address: String,
    pub key: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SocietyActor {
    pub actor_id: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyIdentity {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub balance: i64,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct RecordSocietyUsageInput {
    pub agent_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_number: Option<i64>,
    pub model: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    #[serde(default)]
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyUsageRecord {
    pub id: String,
    pub agent_address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_number: Option<i64>,
    pub model: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd_exact: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyModelUsageTotal {
    pub model: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub cache_write_input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyBudget {
    pub address: String,
    pub balance: i64,
    pub spent_this_window: i64,
    pub window_start: String,
    pub window_end: String,
    pub usage_by_model: Vec<SocietyModelUsageTotal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct AllowanceGrantResult {
    pub window_start: String,
    pub window_end: String,
    pub amount_per_agent: i64,
    pub granted_agents: Vec<String>,
    pub ledger_entries: Vec<ObolLedgerEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[specta(export)]
pub enum SocietyAgentPhase {
    Awake,
    Sleeping,
}

impl SocietyAgentPhase {
    fn as_str(self) -> &'static str {
        match self {
            Self::Awake => "awake",
            Self::Sleeping => "sleeping",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct WakeStartedInput {
    pub wake_number: i64,
    pub permit_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
#[specta(export)]
pub enum ExperimentInjectionTrigger {
    At { timestamp: String },
    Signal { name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
#[specta(export)]
pub enum ExperimentInjectionRecipients {
    Addresses { addresses: Vec<String> },
    All,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ArmExperimentInjectionInput {
    pub trigger: ExperimentInjectionTrigger,
    pub recipients: ExperimentInjectionRecipients,
    pub message: String,
    pub make_due: bool,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct EmitExperimentSignalInput {
    pub name: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[specta(export)]
pub enum ExperimentInjectionState {
    Armed,
    Fired,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ExperimentInjectionRecipient {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_wake_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ExperimentInjection {
    pub id: String,
    pub trigger: ExperimentInjectionTrigger,
    pub recipients: ExperimentInjectionRecipients,
    pub message: String,
    pub make_due: bool,
    pub state: ExperimentInjectionState,
    pub armed_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fired_at: Option<String>,
    pub deliveries: Vec<ExperimentInjectionRecipient>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ExperimentInjectedMessage {
    pub injection_id: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyWakeStart {
    pub lifecycle: SocietyAgentLifecycle,
    pub injected_messages: Vec<ExperimentInjectedMessage>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SleepStartedInput {
    pub wake_number: i64,
    pub next_wake_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAgentLifecycle {
    pub phase: SocietyAgentPhase,
    pub wake_number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_wake_at: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct WakeDebitReceipt {
    pub wake_number: i64,
    pub priced_obols: i64,
    pub charged_obols: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger_entry: Option<ObolLedgerEntry>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct RecordSocietyTranscriptInput {
    pub agent_address: String,
    pub wake_number: i64,
    pub session_jsonl_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyTranscript {
    pub id: String,
    pub agent_address: String,
    pub wake_number: i64,
    pub session_jsonl_base64: String,
    pub byte_length: i64,
    pub sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyWakeContext {
    pub compute_backing: SocietyComputeBacking,
    pub budget: SocietyBudget,
    pub shared_daily_line: String,
    pub unread_count: i64,
    pub paid_posts: Vec<SocietyWakePaidPost>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[specta(export)]
pub enum SocietyComputeBacking {
    Obols,
    Independent,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyWakePaidPost {
    pub headline: String,
    pub sender: String,
    pub attached_obols: i64,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct DirectoryRegistrationInput {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct DirectoryEntry {
    pub address: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct MintObolsInput {
    pub recipient_address: String,
    pub amount: i64,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct TransferObolsInput {
    pub recipient_address: String,
    pub amount: i64,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct CreateEscrowInput {
    pub recipient_address: String,
    pub amount: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ObolLedgerEntry {
    pub id: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actor_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escrow_id: Option<String>,
    pub amount: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct ObolAccountBalance {
    pub address: String,
    pub balance: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct BankSnapshot {
    pub total_minted: i64,
    pub total_balances: i64,
    pub open_escrows: i64,
    pub accounts: Vec<ObolAccountBalance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct EscrowReadModel {
    pub id: String,
    pub payer_address: String,
    pub recipient_address: String,
    pub amount: i64,
    pub status: String,
    pub resolution_authority: EscrowResolutionAuthority,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[specta(export)]
pub enum EscrowResolutionAuthority {
    Payer,
    RecipientRead,
}

impl EscrowResolutionAuthority {
    fn as_str(self) -> &'static str {
        match self {
            Self::Payer => "payer",
            Self::RecipientRead => "recipient_read",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[specta(export)]
pub enum BoardPostKind {
    Note,
    Finding,
    Question,
    Contract,
    Verdict,
    Commitment,
}

impl BoardPostKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Finding => "finding",
            Self::Question => "question",
            Self::Contract => "contract",
            Self::Verdict => "verdict",
            Self::Commitment => "commitment",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct CreateBoardSpaceInput {
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct BoardSpace {
    pub id: String,
    pub slug: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct CreateBoardPostInput {
    pub space_id: String,
    pub kind: BoardPostKind,
    pub headline: String,
    pub body: BoardPostBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_post_id: Option<String>,
    pub recipient_addresses: Vec<String>,
    pub attached_obols: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
#[specta(export)]
pub enum BoardPostBody {
    Text {
        text: String,
    },
    Link {
        url: String,
    },
    File {
        filename: String,
        #[serde(rename = "mediaType")]
        media_type: String,
        #[serde(rename = "contentBase64")]
        content_base64: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct BoardPost {
    pub id: String,
    pub sequence: i64,
    pub reference: String,
    pub space_id: String,
    pub author_address: String,
    pub kind: BoardPostKind,
    pub headline: String,
    pub sealed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<BoardPostBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_post_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_reference: Option<String>,
    pub recipient_addresses: Vec<String>,
    pub attached_obols: i64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct BoardFeed {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<i64>,
    pub posts: Vec<BoardPost>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct BoardReadReceipt {
    pub post_id: String,
    pub post: BoardPost,
    pub cursor: i64,
    pub settled_obols: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminOverview {
    pub agents: Vec<SocietyAdminAgentSummary>,
    pub bank: SocietyAdminBankSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commons: Option<BoardSpace>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub newspaper: Option<SocietyNewspaper>,
    pub posts: Vec<BoardPost>,
    pub ledger: Vec<SocietyAdminLedgerEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminAgentSummary {
    pub address: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub balance: i64,
    pub spent_this_window: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_wake_number: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_wake_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_wake_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_activity_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle: Option<SocietyAgentLifecycle>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminBankSummary {
    pub treasury_address: String,
    pub treasury_balance: i64,
    pub total_minted: i64,
    pub window_start: String,
    pub window_end: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminLedgerEntry {
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_address: Option<String>,
    pub amount: i64,
    pub reason: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminAgentDetail {
    pub agent: SocietyAdminAgentSummary,
    pub wakes: Vec<SocietyAdminWakeReceipt>,
    pub posts: Vec<SocietyAdminAuthoredPost>,
    pub transcripts: Vec<SocietyAdminTranscript>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminWakeReceipt {
    pub wake_number: i64,
    pub model: String,
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub charged_obols: i64,
    pub ending: String,
    pub ended_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminAuthoredPost {
    pub space_name: String,
    pub post: BoardPost,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminTranscript {
    pub wake_number: i64,
    pub created_at: String,
    pub entries: Vec<SocietyAdminTranscriptEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyAdminTranscriptEntry {
    pub role: SocietyAdminTranscriptRole,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    pub is_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub enum SocietyAdminTranscriptRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRecord {
    pub actor_id: String,
    pub username: String,
    pub password_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisDoc {
    pub space_id: String,
    pub title: String,
    pub version: i64,
    pub compiled_at: String,
    pub nodes: Vec<DocNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum DocNode {
    Heading {
        id: String,
        text: String,
    },
    Prose {
        id: String,
        text: String,
    },
    Entity(EntityNode),
    Commit {
        id: String,
        label: String,
        author: String,
        summary: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct EntityNode {
    pub id: String,
    pub role: EntityRole,
    pub state: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub posterior: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markets: Option<Vec<ExternalMarket>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub half_life_days: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dissent: Option<bool>,
    pub holders: Vec<PositionHolder>,
    pub neighbors: Vec<Neighbor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum EntityRole {
    Issue,
    Goal,
    Action,
    Condition,
}

impl EntityRole {
    fn ontology_role(&self) -> &'static str {
        match self {
            Self::Issue => "issue",
            Self::Goal => "goal",
            Self::Action => "action",
            Self::Condition => "condition",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateEntityInput {
    pub role: EntityRole,
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct EditEntityTextInput {
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ChangeEntityStateInput {
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Document,
    Comment,
    Text,
}

impl InputKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Document => "document",
            Self::Comment => "comment",
            Self::Text => "text",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct InputAnchor {
    pub artifact_id: String,
    pub quote: String,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct SubmitInput {
    pub kind: InputKind,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<InputAnchor>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct PublicObjectionInput {
    pub report_artifact_id: String,
    pub report_version: i64,
    pub start: usize,
    pub end: usize,
    pub quote: String,
    pub objection: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ObjectionAnchor {
    pub report_artifact_id: String,
    pub report_version: i64,
    pub start: usize,
    pub end: usize,
    pub quote: String,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct InputReceipt {
    pub artifact_id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LessWrongAuthorInput {
    pub external_id: String,
    pub display_name: String,
    pub slug: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LessWrongMessageInput {
    pub external_id: String,
    pub parent_external_id: Option<String>,
    pub author: LessWrongAuthorInput,
    pub content: String,
    pub posted_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct LessWrongThreadInput {
    pub post_id: String,
    pub title: String,
    pub slug: String,
    pub url: String,
    pub author: LessWrongAuthorInput,
    pub content: String,
    pub posted_at: String,
    pub comments: Vec<LessWrongMessageInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadIngestContext {
    pub space_id: String,
    pub space_title: String,
    pub thread_url: String,
    pub title: String,
    pub standing_positions: Vec<StandingPosition>,
    pub new_sources: Vec<ImportedSource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StandingPosition {
    pub entity_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ImportedSource {
    pub artifact_id: String,
    pub thread_artifact_id: String,
    pub source_kind: String,
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    pub source_url: String,
    pub posted_at: String,
    pub parent_artifact_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadStanceProposal {
    pub actor_id: String,
    pub value: String,
    pub evidence_artifact_id: String,
    pub stated_confidence: Option<String>,
    #[serde(default)]
    pub falsifiers: Vec<ThreadFalsifierProposal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadFalsifierProposal {
    pub quote: String,
    pub condition: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadPositionProposal {
    pub action: String,
    pub target_entity_id: Option<String>,
    pub key: String,
    pub text: String,
    pub reasoning: String,
    pub source_artifact_ids: Vec<String>,
    pub stances: Vec<ThreadStanceProposal>,
}

pub fn validate_thread_position_proposals(
    context: &ThreadIngestContext,
    positions: &[ThreadPositionProposal],
) -> anyhow::Result<()> {
    if positions.len() > 6 {
        return Err(StoreError::Validation(
            "landscape delta must contain at most 6 coarse findings".to_string(),
        )
        .into());
    }
    let source_ids = context
        .new_sources
        .iter()
        .map(|source| source.artifact_id.as_str())
        .collect::<BTreeSet<_>>();
    let standing_ids = context
        .standing_positions
        .iter()
        .map(|position| position.entity_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut keys = BTreeSet::new();
    let mut stance_evidence_keys = BTreeSet::new();
    let mut falsifier_keys = BTreeSet::new();
    for position in positions {
        validate_non_empty("position.key", &position.key)?;
        validate_non_empty("position.text", &position.text)?;
        validate_non_empty("position.reasoning", &position.reasoning)?;
        match position.action.as_str() {
            "new" if position.target_entity_id.is_none() => {}
            "attach"
                if position
                    .target_entity_id
                    .as_deref()
                    .is_some_and(|id| standing_ids.contains(id)) => {}
            _ => {
                return Err(StoreError::Validation(
                    "delta findings must be new without a target or attach to a standing position"
                        .to_string(),
                )
                .into());
            }
        }
        if !keys.insert(position.key.as_str()) {
            return Err(
                StoreError::Validation("thread position keys must be unique".to_string()).into(),
            );
        }
        if position.source_artifact_ids.is_empty()
            || position
                .source_artifact_ids
                .iter()
                .any(|id| !source_ids.contains(id.as_str()))
        {
            return Err(StoreError::Validation(
                "thread positions must cite imported source artifacts".to_string(),
            )
            .into());
        }
        let position_identity = position
            .target_entity_id
            .as_deref()
            .unwrap_or(position.key.as_str());
        if position.stances.iter().any(|stance| {
            let evidence = context.new_sources.iter().find(|source| {
                source.artifact_id == stance.evidence_artifact_id
                    && position.source_artifact_ids.contains(&source.artifact_id)
            });
            !matches!(stance.value.as_str(), "endorse" | "dispute")
                || evidence.is_none_or(|source| source.author_id != stance.actor_id)
                || stance.stated_confidence.as_ref().is_some_and(|confidence| {
                    confidence.trim().is_empty()
                        || confidence.trim() != confidence
                        || !looks_like_stated_confidence(confidence)
                        || evidence.is_none_or(|source| !source.content.contains(confidence))
                })
                || stance.falsifiers.len() > 3
                || stance.falsifiers.iter().any(|falsifier| {
                    falsifier.quote.trim().is_empty()
                        || falsifier.quote.trim() != falsifier.quote
                        || falsifier.condition.trim().is_empty()
                        || falsifier.condition.trim() != falsifier.condition
                        || evidence.is_none_or(|source| !source.content.contains(&falsifier.quote))
                })
        }) {
            return Err(StoreError::Validation(
                "thread stances must cite one exact source by that actor; stated confidence and falsifier quotes must be literal source substrings"
                    .to_string(),
            )
            .into());
        }
        for stance in &position.stances {
            if !stance_evidence_keys.insert((
                position_identity,
                stance.actor_id.as_str(),
                stance.evidence_artifact_id.as_str(),
            )) {
                return Err(StoreError::Validation(
                    "duplicate stance evidence cannot create stance history".to_string(),
                )
                .into());
            }
            for falsifier in &stance.falsifiers {
                if !falsifier_keys.insert((
                    position_identity,
                    stance.actor_id.as_str(),
                    stance.evidence_artifact_id.as_str(),
                    falsifier.quote.as_str(),
                    falsifier.condition.as_str(),
                )) {
                    return Err(StoreError::Validation(
                        "duplicate falsifier evidence cannot create contracts".to_string(),
                    )
                    .into());
                }
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ThreadImportReceipt {
    pub artifact_id: String,
    pub ingest_job_id: Option<String>,
    pub new_source_count: usize,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct WorkerJobReceipt {
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum SynthesisMode {
    Full,
    Selective,
}

impl SynthesisMode {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Selective => "selective",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "lowercase")]
#[specta(export)]
pub enum SynthesizeInput {
    Full,
    Selective { instruction: String },
}

impl SynthesizeInput {
    fn mode(&self) -> SynthesisMode {
        match self {
            Self::Full => SynthesisMode::Full,
            Self::Selective { .. } => SynthesisMode::Selective,
        }
    }

    fn instruction(&self) -> Option<&str> {
        match self {
            Self::Full => None,
            Self::Selective { instruction } => Some(instruction),
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WriteReportInput {
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisReport {
    pub artifact_id: String,
    pub space_id: String,
    pub version: i64,
    pub content: String,
    pub mode: SynthesisMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_reason: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub provenance_anchors: Vec<ReportProvenanceAnchor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReportProvenanceAnchor {
    pub id: String,
    pub start: usize,
    pub end: usize,
    pub entity_id: String,
    pub sources: Vec<SourceLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct PositionHolder {
    pub actor_id: String,
    pub actor_name: String,
    pub stance: String,
    #[serde(default)]
    pub accepted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stated_confidence: Option<String>,
    pub evidence: Vec<SourceLink>,
    #[serde(default)]
    pub history: Vec<HistoricalStance>,
    #[serde(default)]
    pub conditions: Vec<HolderCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalStance {
    pub stance: String,
    pub accepted_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stated_confidence: Option<String>,
    pub evidence: Vec<SourceLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SourceLink {
    pub artifact_id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authored_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct HolderCondition {
    pub contract_id: String,
    pub kind: ContractKind,
    pub terms: String,
    pub condition_entity_id: String,
    pub condition_text: String,
    pub evidence: SourceLink,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct FalsifierGroup {
    pub position_id: String,
    pub position_text: String,
    pub falsifiers: Vec<FalsifierReadModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct FalsifierReadModel {
    pub contract_id: String,
    pub kind: ContractKind,
    pub actor_id: String,
    pub actor_name: String,
    pub terms: String,
    pub condition_entity_id: String,
    pub condition_text: String,
    pub evidence: SourceLink,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub enum ContractKind {
    Falsifier,
    ConditionalStance,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct DeclareConditionInput {
    pub target_entity_id: String,
    pub terms: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactReadModel {
    pub artifact_id: String,
    pub kind: String,
    pub media_type: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReportCause {
    pub event_id: String,
    pub kind: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unverified_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objection_anchor: Option<ObjectionAnchor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_change: Option<EntityStateChange>,
    #[serde(skip)]
    pub input_text: Option<String>,
    #[serde(skip)]
    pub attributed_claim: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub change_classification: Option<ChangeClassification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub enum ChangeClassification {
    #[serde(rename = "strengthened")]
    Strengthened,
    #[serde(rename = "weakened")]
    Weakened,
    #[serde(rename = "new-tension")]
    NewTension,
    #[serde(rename = "redundant")]
    Redundant,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct EntityStateChange {
    pub old_state: String,
    pub new_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Context,
    Added,
    Removed,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct DiffLine {
    pub kind: DiffKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReportVersion {
    #[serde(flatten)]
    pub report: SynthesisReport,
    pub caused_by: Vec<ReportCause>,
    pub diff_from_previous: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReportDiffHunk {
    pub lines: Vec<DiffLine>,
    pub caused_by: Vec<ReportCause>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReportVersionDiff {
    pub version: i64,
    pub previous_version: Option<i64>,
    pub hunks: Vec<ReportDiffHunk>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub enum FeedEntryKind {
    #[serde(rename = "version-written")]
    VersionWritten,
    #[serde(rename = "cause-accepted")]
    CauseAccepted,
    #[serde(rename = "objection-resolved")]
    ObjectionResolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum ObjectionOutcome {
    Accepted,
    Rejected,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct FeedEntry {
    pub event_id: String,
    pub kind: FeedEntryKind,
    pub label: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_version: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_artifact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub causes: Vec<ReportCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<ReportCause>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objection_outcome: Option<ObjectionOutcome>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct PutRefreshScheduleInput {
    pub cadence_seconds: i64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct RefreshSchedule {
    pub space_id: String,
    pub cadence_seconds: i64,
    pub next_due_at: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct IngestContext {
    pub artifact_id: String,
    pub space_id: String,
    pub space_title: String,
    pub input_kind: InputKind,
    pub content: String,
    pub anchor: Option<InputAnchor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_excerpt: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisContext {
    pub space_id: String,
    pub title: String,
    pub compiled_document: SynthesisDoc,
    pub sources: Vec<SynthesisSource>,
    pub causes: Vec<ReportCause>,
    pub current_report: Option<SynthesisReport>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SynthesisSource {
    pub artifact_id: String,
    pub input_kind: String,
    pub content: String,
    pub author_name: Option<String>,
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
pub struct ExternalMarket {
    pub name: String,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[specta(export)]
pub struct Neighbor {
    pub projection: Projection,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewItem {
    pub id: String,
    pub kind: ReviewKind,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_artifact_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unverified_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub objection_anchor: Option<ObjectionAnchor>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_session: Option<ReviewSessionProvenance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum ReviewKind {
    Session,
    Entity,
    Ingest,
    Objection,
    Lint,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct ReviewSessionProvenance {
    pub anchor_block_id: String,
    pub anchor_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stance: Option<Stance>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum Stance {
    Believe,
    Dispute,
}

impl Stance {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Believe => "believe",
            Self::Dispute => "dispute",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionInput {
    pub anchor_block_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stance: Option<Stance>,
}

#[derive(Debug, Clone, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct AppendMessageInput {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SessionReadModel {
    pub id: String,
    pub anchor: SessionAnchor,
    pub turns: Vec<SessionTurn>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stance: Option<Stance>,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SessionAnchor {
    pub block_id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct SessionTurn {
    pub id: String,
    pub who: TurnAuthor,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
#[serde(rename_all = "lowercase")]
pub enum TurnAuthor {
    You,
    Agent,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkerJob {
    pub id: String,
    pub kind: String,
    pub payload_json: String,
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkerAttempt {
    pub id: String,
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct CostTotals {
    pub llm_calls: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct SpaceCostTotals {
    pub space_id: String,
    #[serde(flatten)]
    pub totals: CostTotals,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct StageCostTotals {
    pub stage: String,
    #[serde(flatten)]
    pub totals: CostTotals,
}

#[derive(Debug, Clone, Serialize, Type, PartialEq)]
#[specta(export)]
#[serde(rename_all = "camelCase")]
pub struct CostSummary {
    pub overall: CostTotals,
    pub spaces: Vec<SpaceCostTotals>,
    pub stages: Vec<StageCostTotals>,
    pub served_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentReplyContext {
    pub thread_id: String,
    pub anchor_title: String,
    pub anchor_text: String,
    pub quote: Option<String>,
    pub stance: Option<Stance>,
    pub surrounding_doc: String,
    pub prior_messages: Vec<AgentReplyMessage>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AgentReplyMessage {
    pub who: TurnAuthor,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    Validation(String),
    NotFound(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(message) | Self::NotFound(message) => formatter.write_str(message),
        }
    }
}

impl Error for StoreError {}

pub fn initialize(root: impl AsRef<Path>) -> anyhow::Result<StoreInfo> {
    let paths = StorePaths::from_root(root);

    std::fs::create_dir_all(&paths.root)
        .with_context(|| format!("failed to create data directory {}", paths.root.display()))?;

    let applied_core = migrate_database("core", &paths.core, CORE_MIGRATIONS)?;
    migrate_database("events", &paths.events, EVENTS_MIGRATIONS)?;
    migrate_database("operations", &paths.operations, OPERATIONS_MIGRATIONS)?;

    let store = StoreInfo {
        status: "ready",
        root: paths.root,
        core_path: paths.core,
        events_path: paths.events,
        operations_path: paths.operations,
    };
    if applied_core.contains(&5) {
        store.rebuild_compiled_stance_projections()?;
    }
    Ok(store)
}

impl StorePaths {
    pub fn from_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            core: root.join("core.sqlite"),
            events: root.join("events.sqlite"),
            operations: root.join("operations.sqlite"),
            root,
        }
    }
}

impl StoreInfo {
    pub fn register_actor(&self, username: &str, password_hash: &str) -> anyhow::Result<AuthActor> {
        validate_non_empty("username", username)?;
        validate_non_empty("password_hash", password_hash)?;
        let username = username.trim();
        let actor_id = new_id("actor");
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into actor (id, kind, keys_json) values (?1, 'human', ?2)",
            params![actor_id, json!({ "username": username }).to_string()],
        )?;
        transaction.execute(
            "insert into operations.credential
                (actor_id, username, password_hash, created_at, updated_at)
             values (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![actor_id, username, password_hash],
        )?;
        transaction.commit()?;
        Ok(AuthActor {
            actor_id,
            username: username.to_string(),
        })
    }

    pub fn create_society_agent(
        &self,
        address: &str,
        key_hash: &str,
    ) -> anyhow::Result<SocietyActor> {
        validate_society_address(address)?;
        validate_non_empty("key_hash", key_hash)?;
        let address = address.trim().to_ascii_lowercase();
        let actor_id = new_id("actor");
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        transaction
            .execute(
                "insert into actor (id, kind, keys_json, address) values (?1, 'agent', ?2, ?3)",
                params![
                    actor_id,
                    json!({ "societyBearerSha256": key_hash }).to_string(),
                    address
                ],
            )
            .map_err(|error| match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::ConstraintViolation) => {
                    StoreError::Validation("society address is already registered".to_string())
                        .into()
                }
                _ => anyhow::Error::from(error),
            })?;
        append_event(
            &transaction,
            None,
            Some(&actor_id),
            "society_agent_created",
            json!({ "address": address }),
        )?;
        transaction.commit()?;
        Ok(SocietyActor { actor_id, address })
    }

    pub fn ensure_society_treasury(&self, address: &str) -> anyhow::Result<SocietyActor> {
        validate_society_address(address)?;
        let address = address.trim().to_ascii_lowercase();
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "select id from actor where kind = 'treasury' and address = ?1",
                params![address],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let actor_id = if let Some(actor_id) = existing {
            actor_id
        } else {
            let actor_id = new_id("actor");
            transaction.execute(
                "insert into actor (id, kind, keys_json, address, name) values (?1, 'treasury', '{}', ?2, 'Treasury')",
                params![actor_id, address],
            )?;
            append_event(
                &transaction,
                None,
                Some(&actor_id),
                "society_treasury_created",
                json!({ "address": address }),
            )?;
            actor_id
        };
        transaction.commit()?;
        Ok(SocietyActor { actor_id, address })
    }

    pub fn birth_society_agent(
        &self,
        address: &str,
        name: &str,
        key_hash: &str,
        weekly_allowance: i64,
    ) -> anyhow::Result<SocietyActor> {
        validate_society_address(address)?;
        validate_directory_name(name)?;
        validate_non_empty("key_hash", key_hash)?;
        validate_positive_obols(weekly_allowance)?;
        let address = address.trim().to_ascii_lowercase();
        let name = name.trim().to_string();
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let existing = transaction
            .query_row(
                "select id, name, json_extract(keys_json, '$.societyBearerSha256')
                 from actor where kind = 'agent' and address = ?1",
                params![address],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((actor_id, existing_name, existing_key_hash)) = existing {
            if existing_name == name && existing_key_hash == key_hash {
                transaction.commit()?;
                return Ok(SocietyActor { actor_id, address });
            }
            return Err(StoreError::Validation(
                "society address is already registered with different birth data".to_string(),
            )
            .into());
        }
        let key_owner = transaction
            .query_row(
                "select address from actor
                 where kind = 'agent'
                   and json_extract(keys_json, '$.societyBearerSha256') = ?1",
                params![key_hash],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        if key_owner.is_some() {
            return Err(StoreError::Validation(
                "society key is already registered to another address".to_string(),
            )
            .into());
        }
        let actor_id = new_id("actor");
        transaction
            .execute(
                "insert into actor (id, kind, keys_json, address, name) values (?1, 'agent', ?2, ?3, ?4)",
                params![actor_id, json!({ "societyBearerSha256": key_hash }).to_string(), address, name],
            )
            .map_err(|error| match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::ConstraintViolation) => StoreError::Validation(
                    "society address is already registered".to_string(),
                )
                .into(),
                _ => anyhow::Error::from(error),
            })?;
        append_event(
            &transaction,
            None,
            Some(&actor_id),
            "society_agent_created",
            json!({ "address": address }),
        )?;
        append_event(
            &transaction,
            Some(&actor_id),
            Some(&actor_id),
            "directory_registered",
            json!({ "address": address }),
        )?;
        let (window_start, window_end) = current_week_window(&transaction)?;
        grant_allowance_to_actor(
            &transaction,
            &actor_id,
            weekly_allowance,
            &window_start,
            &window_end,
        )?;
        assert_bank_conservation(&transaction)?;
        transaction.commit()?;
        Ok(SocietyActor { actor_id, address })
    }

    pub fn society_actor_for_key_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<SocietyActor>> {
        let connection = self.open_workspace_connection()?;
        connection
            .query_row(
                "select id, address from actor
                 where kind = 'agent'
                   and address is not null
                   and json_extract(keys_json, '$.societyBearerSha256') = ?1",
                params![key_hash],
                |row| {
                    Ok(SocietyActor {
                        actor_id: row.get(0)?,
                        address: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn read_society_identity(&self, actor_id: &str) -> anyhow::Result<SocietyIdentity> {
        let connection = self.open_workspace_connection()?;
        let (address, name, description) = connection
            .query_row(
                "select address, name, description from actor
                 where id = ?1 and kind = 'agent' and address is not null",
                params![actor_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("society agent not found".to_string()))?;
        let balance = bank_snapshot(&connection)?
            .accounts
            .into_iter()
            .find(|account| account.address == address)
            .map(|account| account.balance)
            .unwrap_or_default();
        Ok(SocietyIdentity {
            address,
            name,
            description,
            balance,
        })
    }

    pub fn register_directory_entry(
        &self,
        actor_id: &str,
        input: DirectoryRegistrationInput,
    ) -> anyhow::Result<DirectoryEntry> {
        validate_directory_name(&input.name)?;
        let name = input.name.trim().to_string();
        let description = normalized_description(input.description)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let address = actor_address(&transaction, actor_id)?;
        let changed = transaction.execute(
            "update actor set name = ?2, description = ?3
             where id = ?1 and kind = 'agent' and name is null",
            params![actor_id, name, description],
        )?;
        if changed != 1 {
            return Err(StoreError::Validation(
                "agent already has a directory registration".to_string(),
            )
            .into());
        }
        append_event(
            &transaction,
            Some(actor_id),
            Some(actor_id),
            "directory_registered",
            json!({ "address": address }),
        )?;
        transaction.commit()?;
        Ok(DirectoryEntry {
            address,
            name,
            description,
        })
    }

    pub fn read_directory(&self) -> anyhow::Result<Vec<DirectoryEntry>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select address, name, description from actor
             where kind = 'agent' and address is not null and name is not null
             order by address",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(DirectoryEntry {
                address: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn mint_obols(
        &self,
        operator_actor_id: Option<&str>,
        input: MintObolsInput,
    ) -> anyhow::Result<ObolLedgerEntry> {
        validate_positive_obols(input.amount)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let recipient_id = actor_id_for_address(&transaction, &input.recipient_address)?;
        let ledger_id = append_ledger_entry(
            &transaction,
            "mint",
            LedgerEntryInput {
                actor_id: None,
                recipient_id: Some(&recipient_id),
                escrow_id: None,
                amount: input.amount,
                reference_id: None,
                resolution_authority: None,
            },
        )?;
        append_event(
            &transaction,
            operator_actor_id,
            Some(&recipient_id),
            "obols_minted",
            json!({ "ledgerId": ledger_id, "amount": input.amount }),
        )?;
        assert_bank_conservation(&transaction)?;
        let entry = read_ledger_entry(&transaction, &ledger_id)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn grant_weekly_allowances(
        &self,
        amount_per_agent: i64,
        treasury_address: &str,
    ) -> anyhow::Result<AllowanceGrantResult> {
        validate_positive_obols(amount_per_agent)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let (window_start, window_end) = current_week_window(&transaction)?;
        let treasury_id = actor_id_for_address(&transaction, treasury_address)?;
        let agents = {
            let mut statement = transaction.prepare(
                "select id, address from actor where kind = 'agent' and address is not null order by address",
            )?;
            statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut granted_agents = Vec::new();
        let mut ledger_entries = Vec::new();
        for (actor_id, address) in agents {
            let existing = transaction
                .query_row(
                    "select ledger_id from operations.society_allowance_grant where actor_id = ?1 and window_start = ?2",
                    params![actor_id, window_start],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if existing.is_some() {
                continue;
            }
            expire_prior_allowances(
                &transaction,
                &actor_id,
                &address,
                &treasury_id,
                &window_start,
                &mut ledger_entries,
            )?;
            let entry = grant_allowance_to_actor(
                &transaction,
                &actor_id,
                amount_per_agent,
                &window_start,
                &window_end,
            )?;
            granted_agents.push(address);
            ledger_entries.push(entry);
        }
        assert_bank_conservation(&transaction)?;
        transaction.commit()?;
        Ok(AllowanceGrantResult {
            window_start,
            window_end,
            amount_per_agent,
            granted_agents,
            ledger_entries,
        })
    }

    pub fn transfer_obols(
        &self,
        actor_id: &str,
        input: TransferObolsInput,
    ) -> anyhow::Result<ObolLedgerEntry> {
        validate_positive_obols(input.amount)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let recipient_id = actor_id_for_address(&transaction, &input.recipient_address)?;
        ensure_spendable(&transaction, actor_id, input.amount)?;
        let ledger_id = append_ledger_entry(
            &transaction,
            "transfer",
            LedgerEntryInput {
                actor_id: Some(actor_id),
                recipient_id: Some(&recipient_id),
                escrow_id: None,
                amount: input.amount,
                reference_id: None,
                resolution_authority: None,
            },
        )?;
        append_event(
            &transaction,
            Some(actor_id),
            Some(&recipient_id),
            "obols_transferred",
            json!({ "ledgerId": ledger_id, "amount": input.amount }),
        )?;
        assert_bank_conservation(&transaction)?;
        let entry = read_ledger_entry(&transaction, &ledger_id)?;
        transaction.commit()?;
        Ok(entry)
    }

    pub fn debit_wake(
        &self,
        actor_id: &str,
        treasury_address: &str,
    ) -> anyhow::Result<WakeDebitReceipt> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let lifecycle = read_society_lifecycle(&transaction, actor_id)?.ok_or_else(|| {
            StoreError::Validation("the agent has no awake wake to debit".to_string())
        })?;
        if lifecycle.phase != SocietyAgentPhase::Awake {
            return Err(
                StoreError::Validation("the agent has no awake wake to debit".to_string()).into(),
            );
        }
        let wake_number = lifecycle.wake_number;
        let address = actor_address(&transaction, actor_id)?;
        let treasury_id = actor_id_for_address(&transaction, treasury_address)?;
        if let Some((priced_obols, amount, recorded_treasury, ledger_id)) = transaction
            .query_row(
                "select priced_amount, amount, treasury_actor_id, ledger_id
                 from operations.society_wake_debit where actor_id = ?1 and wake_number = ?2",
                params![actor_id, wake_number],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
        {
            if recorded_treasury != treasury_id {
                return Err(StoreError::Validation(
                    "wake debit already exists with different content".to_string(),
                )
                .into());
            }
            return Ok(WakeDebitReceipt {
                wake_number,
                priced_obols,
                charged_obols: amount,
                ledger_entry: ledger_id
                    .as_deref()
                    .map(|ledger_id| read_ledger_entry(&transaction, ledger_id))
                    .transpose()?,
            });
        }
        let usage = read_society_usage_for_wake(&transaction, actor_id, &address, wake_number)?
            .ok_or_else(|| {
                StoreError::Validation("the awake wake has no recorded usage".to_string())
            })?;
        let priced_obols = priced_wake_obols(&usage)?;
        let charged_obols = priced_obols.min(spendable_balance(&transaction, actor_id)?);
        let ledger_id = if charged_obols > 0 {
            let reference = format!("wake:{address}:{wake_number}");
            Some(append_ledger_entry(
                &transaction,
                "transfer",
                LedgerEntryInput {
                    actor_id: Some(actor_id),
                    recipient_id: Some(&treasury_id),
                    escrow_id: None,
                    amount: charged_obols,
                    reference_id: Some(&reference),
                    resolution_authority: None,
                },
            )?)
        } else {
            None
        };
        transaction.execute(
            "insert into operations.society_wake_debit
                (actor_id, wake_number, priced_amount, amount, treasury_actor_id, ledger_id, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                actor_id,
                wake_number,
                priced_obols,
                charged_obols,
                treasury_id,
                ledger_id.as_deref()
            ],
        )?;
        append_event(
            &transaction,
            Some(actor_id),
            Some(&treasury_id),
            "society_wake_debited",
            json!({
                "ledgerId": ledger_id,
                "wakeNumber": wake_number,
                "pricedObols": priced_obols,
                "chargedObols": charged_obols,
            }),
        )?;
        assert_bank_conservation(&transaction)?;
        let entry = ledger_id
            .as_deref()
            .map(|ledger_id| read_ledger_entry(&transaction, ledger_id))
            .transpose()?;
        transaction.commit()?;
        Ok(WakeDebitReceipt {
            wake_number,
            priced_obols,
            charged_obols,
            ledger_entry: entry,
        })
    }

    pub fn create_escrow(
        &self,
        actor_id: &str,
        input: CreateEscrowInput,
    ) -> anyhow::Result<EscrowReadModel> {
        validate_positive_obols(input.amount)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let recipient_id = actor_id_for_address(&transaction, &input.recipient_address)?;
        let escrow_id = create_escrow_in_transaction(
            &transaction,
            actor_id,
            &recipient_id,
            input.amount,
            input.reference_id.as_deref(),
            EscrowResolutionAuthority::Payer,
        )?;
        assert_bank_conservation(&transaction)?;
        let escrow = read_escrow(&transaction, &escrow_id)?;
        transaction.commit()?;
        Ok(escrow)
    }

    pub fn release_escrow(
        &self,
        actor_id: &str,
        escrow_id: &str,
    ) -> anyhow::Result<EscrowReadModel> {
        self.finish_escrow(actor_id, escrow_id, "escrow_release")
    }

    pub fn execute_escrow(
        &self,
        actor_id: &str,
        escrow_id: &str,
    ) -> anyhow::Result<EscrowReadModel> {
        self.finish_escrow(actor_id, escrow_id, "escrow_execute")
    }

    fn finish_escrow(
        &self,
        actor_id: &str,
        escrow_id: &str,
        kind: &str,
    ) -> anyhow::Result<EscrowReadModel> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        finish_escrow_in_transaction(
            &transaction,
            EscrowResolutionAttempt::Payer(actor_id),
            escrow_id,
            kind,
        )?;
        assert_bank_conservation(&transaction)?;
        let escrow = read_escrow(&transaction, escrow_id)?;
        transaction.commit()?;
        Ok(escrow)
    }

    pub fn read_bank_snapshot(&self) -> anyhow::Result<BankSnapshot> {
        let connection = self.open_workspace_connection()?;
        bank_snapshot(&connection)
    }

    pub fn read_obol_ledger(&self) -> anyhow::Result<Vec<ObolLedgerEntry>> {
        let connection = self.open_workspace_connection()?;
        let mut statement =
            connection.prepare("select id from operations.obol_ledger order by created_at, id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| read_ledger_entry(&connection, id))
            .collect()
    }

    pub fn record_society_usage(
        &self,
        input: RecordSocietyUsageInput,
    ) -> anyhow::Result<SocietyUsageRecord> {
        validate_non_empty("model", &input.model)?;
        if input.model.trim().len() > 200 {
            return Err(
                StoreError::Validation("model must be at most 200 characters".to_string()).into(),
            );
        }
        if input.wake_number.is_some_and(|wake| wake <= 0) {
            return Err(StoreError::Validation("wakeNumber must be positive".to_string()).into());
        }
        validate_usage_tokens("input tokens", input.input_tokens)?;
        validate_usage_tokens("cached input tokens", input.cached_input_tokens)?;
        validate_usage_tokens("cache write input tokens", input.cache_write_input_tokens)?;
        validate_usage_tokens("output tokens", input.output_tokens)?;
        validate_usage_tokens("reasoning output tokens", input.reasoning_output_tokens)?;
        let cost_usd = input
            .cost_usd
            .as_deref()
            .map(validate_exact_usd)
            .transpose()?;

        let connection = self.open_workspace_connection()?;
        let actor_id = actor_id_for_address(&connection, &input.agent_address)?;
        let address = actor_address(&connection, &actor_id)?;
        if let Some(wake_number) = input.wake_number {
            if let Some(existing) =
                read_society_usage_for_wake(&connection, &actor_id, &address, wake_number)?
            {
                let same = existing.model == input.model.trim()
                    && existing.input_tokens == input.input_tokens
                    && existing.cached_input_tokens == input.cached_input_tokens
                    && existing.cache_write_input_tokens == input.cache_write_input_tokens
                    && existing.output_tokens == input.output_tokens
                    && existing.reasoning_output_tokens == input.reasoning_output_tokens
                    && existing.cost_usd_exact == input.cost_usd;
                if same {
                    return Ok(existing);
                }
                return Err(StoreError::Validation(
                    "wake usage already exists with different content".to_string(),
                )
                .into());
            }
        }
        let record = SocietyUsageRecord {
            id: new_id("usage"),
            agent_address: address,
            wake_number: input.wake_number,
            model: input.model.trim().to_string(),
            input_tokens: input.input_tokens,
            cached_input_tokens: input.cached_input_tokens,
            cache_write_input_tokens: input.cache_write_input_tokens,
            output_tokens: input.output_tokens,
            reasoning_output_tokens: input.reasoning_output_tokens,
            cost_usd,
            cost_usd_exact: input.cost_usd,
            created_at: now_label(),
        };
        connection.execute(
            "insert into operations.society_usage
                (id, actor_id, wake_number, model, input_tokens, cached_input_tokens,
                 cache_write_input_tokens, output_tokens, reasoning_output_tokens, cost_usd,
                 cost_usd_exact, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                record.id,
                actor_id,
                record.wake_number,
                record.model,
                record.input_tokens,
                record.cached_input_tokens,
                record.cache_write_input_tokens,
                record.output_tokens,
                record.reasoning_output_tokens,
                record.cost_usd,
                record.cost_usd_exact,
                record.created_at
            ],
        )?;
        Ok(record)
    }

    pub fn read_society_usage(
        &self,
        agent_address: &str,
    ) -> anyhow::Result<Vec<SocietyUsageRecord>> {
        let connection = self.open_workspace_connection()?;
        let actor_id = actor_id_for_address(&connection, agent_address)?;
        let address = actor_address(&connection, &actor_id)?;
        let mut statement = connection.prepare(
            "select id, wake_number, model, input_tokens, cached_input_tokens,
                    cache_write_input_tokens, output_tokens, reasoning_output_tokens, cost_usd,
                    cost_usd_exact, created_at
             from operations.society_usage where actor_id = ?1 order by created_at, id",
        )?;
        let rows = statement.query_map(params![actor_id], |row| {
            Ok(SocietyUsageRecord {
                id: row.get(0)?,
                agent_address: address.clone(),
                wake_number: row.get(1)?,
                model: row.get(2)?,
                input_tokens: row.get(3)?,
                cached_input_tokens: row.get(4)?,
                cache_write_input_tokens: row.get(5)?,
                output_tokens: row.get(6)?,
                reasoning_output_tokens: row.get(7)?,
                cost_usd: row.get(8)?,
                cost_usd_exact: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn record_society_transcript(
        &self,
        input: RecordSocietyTranscriptInput,
    ) -> anyhow::Result<SocietyTranscript> {
        if input.wake_number <= 0 {
            return Err(StoreError::Validation("wakeNumber must be positive".to_string()).into());
        }
        let session = base64::engine::general_purpose::STANDARD
            .decode(&input.session_jsonl_base64)
            .map_err(|_| {
                StoreError::Validation("sessionJsonlBase64 must be valid base64".to_string())
            })?;
        for line in session.split(|byte| *byte == b'\n') {
            if line.is_empty() {
                continue;
            }
            serde_json::from_slice::<Value>(line).map_err(|_| {
                StoreError::Validation("session transcript must be JSONL".to_string())
            })?;
        }
        let connection = self.open_workspace_connection()?;
        let actor_id = actor_id_for_address(&connection, &input.agent_address)?;
        let address = actor_address(&connection, &actor_id)?;
        if let Some(existing) =
            read_society_transcript_for_wake(&connection, &actor_id, &address, input.wake_number)?
        {
            if existing.session_jsonl_base64 == input.session_jsonl_base64 {
                return Ok(existing);
            }
            return Err(StoreError::Validation(
                "wake transcript already exists and cannot be mutated".to_string(),
            )
            .into());
        }
        let digest = format!("{:x}", Sha256::digest(&session));
        let transcript = SocietyTranscript {
            id: new_id("transcript"),
            agent_address: address,
            wake_number: input.wake_number,
            session_jsonl_base64: input.session_jsonl_base64,
            byte_length: i64::try_from(session.len())?,
            sha256: digest,
            created_at: now_label(),
        };
        connection.execute(
            "insert into operations.society_transcript
                (id, actor_id, wake_number, session_jsonl, byte_length, sha256, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                transcript.id,
                actor_id,
                transcript.wake_number,
                session,
                transcript.byte_length,
                transcript.sha256,
                transcript.created_at
            ],
        )?;
        Ok(transcript)
    }

    pub fn read_society_transcripts(
        &self,
        agent_address: &str,
    ) -> anyhow::Result<Vec<SocietyTranscript>> {
        let connection = self.open_workspace_connection()?;
        let actor_id = actor_id_for_address(&connection, agent_address)?;
        let address = actor_address(&connection, &actor_id)?;
        let mut statement = connection.prepare(
            "select id, wake_number, session_jsonl, byte_length, sha256, created_at
             from operations.society_transcript where actor_id = ?1 order by wake_number",
        )?;
        statement
            .query_map(params![actor_id], |row| {
                let bytes = row.get::<_, Vec<u8>>(2)?;
                Ok(SocietyTranscript {
                    id: row.get(0)?,
                    agent_address: address.clone(),
                    wake_number: row.get(1)?,
                    session_jsonl_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    byte_length: row.get(3)?,
                    sha256: row.get(4)?,
                    created_at: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_society_budget(
        &self,
        actor_id: &str,
        treasury_address: &str,
    ) -> anyhow::Result<SocietyBudget> {
        let connection = self.open_workspace_connection()?;
        let address = actor_address(&connection, actor_id)?;
        let treasury_id = actor_id_for_address(&connection, treasury_address)?;
        let balance = bank_snapshot(&connection)?
            .accounts
            .into_iter()
            .find(|account| account.address == address)
            .map(|account| account.balance)
            .unwrap_or_default();
        let (window_start, window_end) = current_week_window(&connection)?;
        let spent_this_window = connection.query_row(
            "select coalesce(sum(amount), 0) from operations.obol_ledger
             where kind = 'transfer' and actor_id = ?1 and recipient_id = ?2
               and reference_id like 'wake:%' and created_at >= ?3 and created_at < ?4",
            params![actor_id, treasury_id, window_start, window_end],
            |row| row.get(0),
        )?;
        let mut statement = connection.prepare(
            "select model, sum(input_tokens), sum(cached_input_tokens),
                    sum(cache_write_input_tokens), sum(output_tokens),
                    sum(reasoning_output_tokens), coalesce(sum(cost_usd), 0.0)
             from operations.society_usage where actor_id = ?1
             group by model order by model",
        )?;
        let usage_by_model = statement
            .query_map(params![actor_id], |row| {
                Ok(SocietyModelUsageTotal {
                    model: row.get(0)?,
                    input_tokens: row.get(1)?,
                    cached_input_tokens: row.get(2)?,
                    cache_write_input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    reasoning_output_tokens: row.get(5)?,
                    cost_usd: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SocietyBudget {
            address,
            balance,
            spent_this_window,
            window_start,
            window_end,
            usage_by_model,
        })
    }

    pub fn record_society_wake_started(
        &self,
        actor_id: &str,
        wake_number: i64,
        require_obol_balance: bool,
    ) -> anyhow::Result<SocietyWakeStart> {
        if wake_number <= 0 {
            return Err(StoreError::Validation("wakeNumber must be positive".to_string()).into());
        }
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = read_society_lifecycle(&transaction, actor_id)?;
        let starts_new_wake = !matches!(
            existing,
            Some(ref lifecycle)
                if lifecycle.phase == SocietyAgentPhase::Awake
                    && lifecycle.wake_number == wake_number
        );
        if starts_new_wake && require_obol_balance && spendable_balance(&transaction, actor_id)? < 1
        {
            return Err(StoreError::Validation(
                "insufficient obol balance to start a backed wake".to_string(),
            )
            .into());
        }
        let lifecycle = record_society_lifecycle_in_transaction(
            &transaction,
            actor_id,
            SocietyAgentPhase::Awake,
            wake_number,
            None,
        )?;
        if starts_new_wake {
            // Close the timestamp boundary inside the same write transaction as assignment.
            // A replay of an already-started wake must not cross that boundary again.
            fire_due_experiment_injections_in_transaction(&transaction)?;
            assign_pending_experiment_injections(&transaction, actor_id, wake_number)?;
        }
        // The assignment is durable and belongs to this wake. A response retry must replay it.
        let injected_messages =
            read_assigned_experiment_injections(&transaction, actor_id, wake_number)?;
        transaction.commit()?;
        Ok(SocietyWakeStart {
            lifecycle,
            injected_messages,
        })
    }

    pub fn read_society_lifecycle(
        &self,
        actor_id: &str,
    ) -> anyhow::Result<Option<SocietyAgentLifecycle>> {
        let connection = self.open_workspace_connection()?;
        read_society_lifecycle(&connection, actor_id)
    }

    pub fn request_society_wake(
        &self,
        agent_address: &str,
    ) -> anyhow::Result<SocietyAgentLifecycle> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let actor_id = actor_id_for_address(&transaction, agent_address)?;
        let existing = read_society_lifecycle(&transaction, &actor_id)?.ok_or_else(|| {
            StoreError::Validation("the agent has no completed wake to resume".to_string())
        })?;
        if existing.phase != SocietyAgentPhase::Sleeping {
            return Err(StoreError::Validation("the agent is already awake".to_string()).into());
        }
        request_society_wake_in_transaction(&transaction, &actor_id, None)?;
        let lifecycle = read_society_lifecycle(&transaction, &actor_id)?
            .ok_or_else(|| anyhow::anyhow!("updated lifecycle is missing"))?;
        transaction.commit()?;
        Ok(lifecycle)
    }

    pub fn read_society_wake_line(&self) -> anyhow::Result<Option<String>> {
        let connection = self.open_workspace_connection()?;
        Ok(connection
            .query_row(
                "select text from operations.society_wake_line where singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub fn put_society_wake_line(&self, text: &str) -> anyhow::Result<String> {
        let text = text.trim();
        if text.len() > 4_000 {
            return Err(
                StoreError::Validation("wake line must not exceed 4000 bytes".to_string()).into(),
            );
        }
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "insert into operations.society_wake_line(singleton, text, updated_at)
             values (1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             on conflict(singleton) do update set text = excluded.text, updated_at = excluded.updated_at",
            params![text],
        )?;
        Ok(text.to_string())
    }

    pub fn record_society_sleep_started(
        &self,
        actor_id: &str,
        input: SleepStartedInput,
    ) -> anyhow::Result<SocietyAgentLifecycle> {
        validate_non_empty("nextWakeAt", &input.next_wake_at)?;
        let mut connection = self.open_workspace_connection()?;
        let valid = connection.query_row(
            "select unixepoch(?1) is not null",
            params![input.next_wake_at],
            |row| row.get::<_, bool>(0),
        )?;
        if !valid {
            return Err(StoreError::Validation(
                "nextWakeAt must be an RFC 3339 timestamp".to_string(),
            )
            .into());
        }
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut lifecycle = record_society_lifecycle_in_transaction(
            &transaction,
            actor_id,
            SocietyAgentPhase::Sleeping,
            input.wake_number,
            Some(input.next_wake_at),
        )?;
        let pending_make_due = transaction.query_row(
            "select exists(
                select 1
                from operations.experiment_injection_recipient recipient
                join operations.experiment_injection injection on injection.id = recipient.injection_id
                where recipient.actor_id = ?1 and recipient.delivered_at is null
                  and injection.state = 'fired' and injection.make_due = 1
             )",
            params![actor_id],
            |row| row.get::<_, bool>(0),
        )?;
        if pending_make_due {
            request_society_wake_in_transaction(&transaction, actor_id, None)?;
            lifecycle = read_society_lifecycle(&transaction, actor_id)?
                .ok_or_else(|| anyhow::anyhow!("updated lifecycle is missing"))?;
        }
        transaction.commit()?;
        Ok(lifecycle)
    }

    pub fn arm_experiment_injection(
        &self,
        input: ArmExperimentInjectionInput,
    ) -> anyhow::Result<ExperimentInjection> {
        validate_injection_message(&input.message)?;
        let (trigger_kind, trigger_value) = validate_injection_trigger(&input.trigger)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recipients = resolve_injection_recipients(&transaction, &input.recipients)?;
        if recipients.is_empty() {
            return Err(StoreError::Validation(
                "an injection must have at least one recipient".to_string(),
            )
            .into());
        }
        let injection_id = new_id("injection");
        let recipient_kind = match &input.recipients {
            ExperimentInjectionRecipients::Addresses { .. } => "addresses",
            ExperimentInjectionRecipients::All => "all",
        };
        transaction.execute(
            "insert into operations.experiment_injection
                (id, trigger_kind, trigger_value, message, recipient_kind, make_due, state, armed_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, 'armed', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                injection_id,
                trigger_kind,
                trigger_value,
                input.message,
                recipient_kind,
                input.make_due,
            ],
        )?;
        for (actor_id, _) in &recipients {
            transaction.execute(
                "insert into operations.experiment_injection_recipient(injection_id, actor_id)
                 values (?1, ?2)",
                params![injection_id, actor_id],
            )?;
        }
        append_event(
            &transaction,
            None,
            None,
            "experiment_injection_armed",
            json!({
                "injectionId": injection_id,
                "trigger": match trigger_kind {
                    "at" => json!({ "type": "at", "timestamp": trigger_value }),
                    "signal" => json!({ "type": "signal", "name": trigger_value }),
                    _ => unreachable!("validated injection trigger kind"),
                },
                "recipients": recipients.iter().map(|(_, address)| address).collect::<Vec<_>>(),
                "recipientKind": recipient_kind,
                "makeDue": input.make_due,
            }),
        )?;
        let injection = read_experiment_injection(&transaction, &injection_id)?;
        transaction.commit()?;
        Ok(injection)
    }

    pub fn read_experiment_injections(&self) -> anyhow::Result<Vec<ExperimentInjection>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection
            .prepare("select id from operations.experiment_injection order by armed_at, id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| read_experiment_injection(&connection, id))
            .collect()
    }

    pub fn emit_experiment_signal(&self, name: &str) -> anyhow::Result<Vec<ExperimentInjection>> {
        validate_signal_name(name)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids = transaction
            .prepare(
                "select id from operations.experiment_injection
             where state = 'armed' and trigger_kind = 'signal' and trigger_value = ?1
             order by id",
            )?
            .query_map(params![name], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let fired = fire_experiment_injections_in_transaction(&transaction, ids)?;
        transaction.commit()?;
        Ok(fired)
    }

    pub fn fire_due_experiment_injections(&self) -> anyhow::Result<Vec<ExperimentInjection>> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let fired = fire_due_experiment_injections_in_transaction(&transaction)?;
        transaction.commit()?;
        Ok(fired)
    }

    pub fn create_board_space(
        &self,
        actor_id: &str,
        input: CreateBoardSpaceInput,
    ) -> anyhow::Result<BoardSpace> {
        validate_board_slug(&input.slug)?;
        validate_non_empty("space name", &input.name)?;
        if input.name.trim().len() > 80 {
            return Err(StoreError::Validation(
                "space name must be at most 80 characters".to_string(),
            )
            .into());
        }
        let space = BoardSpace {
            id: new_id("space"),
            slug: input.slug.trim().to_ascii_lowercase(),
            name: input.name.trim().to_string(),
        };
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        transaction
            .execute(
                "insert into space (id, settings_json) values (?1, ?2)",
                params![
                    space.id,
                    json!({
                        "title": space.name,
                        "projection": "belief",
                        "society": { "slug": space.slug, "createdBy": actor_id }
                    })
                    .to_string()
                ],
            )
            .map_err(|error| match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::ConstraintViolation) => {
                    StoreError::Validation("space slug is already registered".to_string()).into()
                }
                _ => anyhow::Error::from(error),
            })?;
        append_event(
            &transaction,
            Some(actor_id),
            Some(&space.id),
            "board_space_created",
            json!({ "slug": space.slug, "name": space.name }),
        )?;
        transaction.commit()?;
        Ok(space)
    }

    pub fn read_board_spaces(&self) -> anyhow::Result<Vec<BoardSpace>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select id, json_extract(settings_json, '$.society.slug'),
                    json_extract(settings_json, '$.title')
             from space where json_extract(settings_json, '$.society.slug') is not null
             order by json_extract(settings_json, '$.society.slug')",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(BoardSpace {
                id: row.get(0)?,
                slug: row.get(1)?,
                name: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn read_society_admin_overview(
        &self,
        treasury_address: &str,
    ) -> anyhow::Result<SocietyAdminOverview> {
        let connection = self.open_workspace_connection()?;
        let bank_snapshot = bank_snapshot(&connection)?;
        let (window_start, window_end) = current_week_window(&connection)?;
        let treasury_id = actor_id_for_address(&connection, treasury_address)?;
        let mut directory_statement = connection.prepare(
            "select id, address, name, description from actor
             where kind = 'agent' and address is not null and name is not null
             order by address",
        )?;
        let directory = directory_statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let agents = directory
            .into_iter()
            .map(|(actor_id, address, name, description)| {
                read_society_admin_agent_summary(
                    &connection,
                    &bank_snapshot,
                    &treasury_id,
                    &window_start,
                    &window_end,
                    (actor_id, address, name, description),
                )
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let commons = connection
            .query_row(
                "select id, json_extract(settings_json, '$.society.slug'),
                        json_extract(settings_json, '$.title')
                 from space where json_extract(settings_json, '$.society.slug') = 'commons'",
                [],
                |row| {
                    Ok(BoardSpace {
                        id: row.get(0)?,
                        slug: row.get(1)?,
                        name: row.get(2)?,
                    })
                },
            )
            .optional()?;
        let posts = if let Some(space) = &commons {
            let mut statement = connection.prepare(
                "select id from operations.society_post where space_id = ?1 order by sequence",
            )?;
            let ids = statement
                .query_map(params![space.id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            ids.iter()
                .map(|id| read_board_post(&connection, id, None))
                .collect::<anyhow::Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        let newspaper = commons
            .as_ref()
            .map(|space| read_society_newspaper_model(&connection, &space.id))
            .transpose()?
            .flatten();
        let mut ledger_statement = connection
            .prepare("select id from operations.obol_ledger order by created_at desc, id desc")?;
        let ledger_ids = ledger_statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let ledger = ledger_ids
            .iter()
            .map(|id| {
                let entry = read_ledger_entry(&connection, id)?;
                society_admin_ledger_entry(&connection, entry)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let treasury_balance = bank_snapshot
            .accounts
            .iter()
            .find(|account| account.address == treasury_address)
            .map(|account| account.balance)
            .unwrap_or_default();
        Ok(SocietyAdminOverview {
            agents,
            bank: SocietyAdminBankSummary {
                treasury_address: treasury_address.to_string(),
                treasury_balance,
                total_minted: bank_snapshot.total_minted,
                window_start,
                window_end,
            },
            commons,
            newspaper,
            posts,
            ledger,
        })
    }

    pub fn read_society_admin_agent_detail(
        &self,
        treasury_address: &str,
        agent_address: &str,
    ) -> anyhow::Result<SocietyAdminAgentDetail> {
        let connection = self.open_workspace_connection()?;
        let (actor_id, address, name, description) = connection
            .query_row(
                "select id, address, name, description from actor
                 where kind = 'agent' and address = ?1 and name is not null",
                params![agent_address.trim().to_ascii_lowercase()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("society agent not found".to_string()))?;
        let bank_snapshot = bank_snapshot(&connection)?;
        let (window_start, window_end) = current_week_window(&connection)?;
        let treasury_id = actor_id_for_address(&connection, treasury_address)?;
        let agent = read_society_admin_agent_summary(
            &connection,
            &bank_snapshot,
            &treasury_id,
            &window_start,
            &window_end,
            (actor_id.clone(), address.clone(), name, description),
        )?;

        let mut transcript_statement = connection.prepare(
            "select wake_number, session_jsonl, created_at
             from operations.society_transcript where actor_id = ?1 order by wake_number desc",
        )?;
        let transcript_rows = transcript_statement
            .query_map(params![actor_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let transcript_by_wake = transcript_rows
            .iter()
            .map(|(wake, bytes, created_at)| (*wake, (bytes.as_slice(), created_at.as_str())))
            .collect::<BTreeMap<_, _>>();

        let mut usage_statement = connection.prepare(
            "select wake_number, model, input_tokens, cached_input_tokens, output_tokens,
                    cost_usd, created_at
             from operations.society_usage
             where actor_id = ?1 and wake_number is not null order by wake_number desc",
        )?;
        let usage_rows = usage_statement
            .query_map(params![actor_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<f64>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let wakes = usage_rows
            .into_iter()
            .filter_map(
                |(
                    wake_number,
                    model,
                    input_tokens,
                    cached_input_tokens,
                    output_tokens,
                    cost_usd,
                    usage_at,
                )| {
                    let (bytes, transcript_at) = transcript_by_wake.get(&wake_number)?;
                    Some((|| -> anyhow::Result<SocietyAdminWakeReceipt> {
                        let charged_obols = connection
                            .query_row(
                                "select amount from operations.society_wake_debit
                             where actor_id = ?1 and wake_number = ?2",
                                params![actor_id, wake_number],
                                |row| row.get::<_, i64>(0),
                            )
                            .optional()?
                            .unwrap_or_default();
                        let (_, ending) = parse_society_transcript(bytes)?;
                        Ok(SocietyAdminWakeReceipt {
                            wake_number,
                            model,
                            input_tokens,
                            cached_input_tokens,
                            output_tokens,
                            cost_usd,
                            charged_obols,
                            ending,
                            ended_at: std::cmp::max(usage_at, (*transcript_at).to_string()),
                        })
                    })())
                },
            )
            .collect::<anyhow::Result<Vec<_>>>()?;

        let mut post_statement = connection.prepare(
            "select post.id, json_extract(space.settings_json, '$.title')
             from operations.society_post post
             join space on space.id = post.space_id
             where post.author_actor_id = ?1 order by post.sequence desc",
        )?;
        let authored_rows = post_statement
            .query_map(params![actor_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let posts = authored_rows
            .into_iter()
            .map(|(post_id, space_name)| {
                Ok(SocietyAdminAuthoredPost {
                    space_name,
                    post: read_board_post(&connection, &post_id, None)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let transcripts = transcript_rows
            .into_iter()
            .map(|(wake_number, bytes, created_at)| {
                let (entries, _) = parse_society_transcript(&bytes)?;
                Ok(SocietyAdminTranscript {
                    wake_number,
                    created_at,
                    entries,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(SocietyAdminAgentDetail {
            agent,
            wakes,
            posts,
            transcripts,
        })
    }

    pub fn create_board_post(
        &self,
        actor_id: &str,
        input: CreateBoardPostInput,
    ) -> anyhow::Result<BoardPost> {
        validate_headline(&input.headline)?;
        validate_board_post_body(&input.body)?;
        if input.attached_obols < 0 {
            return Err(
                StoreError::Validation("attached obols cannot be negative".to_string()).into(),
            );
        }
        let recipient_addresses = input
            .recipient_addresses
            .iter()
            .map(|address| address.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        if input.attached_obols > 0 && recipient_addresses.is_empty() {
            return Err(StoreError::Validation(
                "attached obols require at least one addressed recipient".to_string(),
            )
            .into());
        }
        let total_attached = input
            .attached_obols
            .checked_mul(recipient_addresses.len() as i64)
            .ok_or_else(|| {
                StoreError::Validation("attached obol total is too large".to_string())
            })?;

        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let space_exists = transaction
            .query_row(
                "select 1 from space
                 where id = ?1 and json_extract(settings_json, '$.society.slug') is not null",
                params![input.space_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !space_exists {
            return Err(StoreError::NotFound("board space not found".to_string()).into());
        }
        let parent_post_id = input
            .parent_post_id
            .as_deref()
            .map(|reference| resolve_board_post_id(&transaction, reference))
            .transpose()?;
        if let Some(parent_id) = parent_post_id.as_deref() {
            let parent_space = transaction
                .query_row(
                    "select space_id from operations.society_post where id = ?1",
                    params![parent_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
                .ok_or_else(|| StoreError::NotFound("parent post not found".to_string()))?;
            if parent_space != input.space_id {
                return Err(StoreError::Validation(
                    "reply parent must be in the same space".to_string(),
                )
                .into());
            }
        }
        if total_attached > 0 {
            ensure_spendable(&transaction, actor_id, total_attached)?;
        }
        let recipient_ids = recipient_addresses
            .iter()
            .map(|address| {
                Ok((
                    address.clone(),
                    actor_id_for_address(&transaction, address)?,
                ))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let post_id = new_id("post");
        let artifact_id = new_id("artifact");
        let (media_type, content, body_metadata) = board_post_artifact(&input.body);
        transaction.execute(
            "insert into artifact (id, kind, media_type, content, derived_from, metadata_json)
             values (?1, 'board_post', ?2, ?3, null, ?4)",
            params![
                artifact_id,
                media_type,
                content,
                json!({ "boardPostId": post_id, "body": body_metadata }).to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.society_post
                (id, space_id, author_actor_id, kind, headline, artifact_id, parent_post_id, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                post_id,
                input.space_id,
                actor_id,
                input.kind.as_str(),
                input.headline.trim(),
                artifact_id,
                parent_post_id
            ],
        )?;
        for (_, recipient_id) in &recipient_ids {
            transaction.execute(
                "insert into operations.society_post_recipient (post_id, actor_id) values (?1, ?2)",
                params![post_id, recipient_id],
            )?;
            let escrow_id = if input.attached_obols > 0 {
                Some(create_escrow_in_transaction(
                    &transaction,
                    actor_id,
                    recipient_id,
                    input.attached_obols,
                    Some(&post_id),
                    EscrowResolutionAuthority::RecipientRead,
                )?)
            } else {
                None
            };
            transaction.execute(
                "insert into operations.society_attention
                    (post_id, recipient_actor_id, amount, escrow_id)
                 values (?1, ?2, ?3, ?4)",
                params![post_id, recipient_id, input.attached_obols, escrow_id],
            )?;
        }
        append_event(
            &transaction,
            Some(actor_id),
            Some(&post_id),
            "board_posted",
            json!({
                "spaceId": input.space_id,
                "artifactId": artifact_id,
                "kind": input.kind.as_str(),
                "headline": input.headline.trim(),
                "parentPostId": parent_post_id,
                "recipientAddresses": recipient_addresses,
                "attachedObolsPerRecipient": input.attached_obols
            }),
        )?;
        if recipient_addresses.is_empty() {
            enqueue_newspaper_job(&transaction, actor_id, &input.space_id, true)?;
        }
        assert_bank_conservation(&transaction)?;
        let post = read_board_post(&transaction, &post_id, Some(actor_id))?;
        transaction.commit()?;
        Ok(post)
    }

    pub fn read_board_feed(&self, actor_id: &str, space_id: &str) -> anyhow::Result<BoardFeed> {
        let connection = self.open_workspace_connection()?;
        let cursor = connection
            .query_row(
                "select post_sequence from operations.society_read_cursor
                 where actor_id = ?1 and space_id = ?2",
                params![actor_id, space_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let space_exists = connection
            .query_row(
                "select 1 from space
                 where id = ?1 and json_extract(settings_json, '$.society.slug') is not null",
                params![space_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !space_exists {
            return Err(StoreError::NotFound("board space not found".to_string()).into());
        }
        let mut statement = connection.prepare(
            "select post.id
             from operations.society_post post
             where post.space_id = ?1
               and post.sequence > ?2
               and (
                   post.author_actor_id = ?3
                   or not exists (
                       select 1 from operations.society_post_recipient recipient
                       where recipient.post_id = post.id
                   )
                   or exists (
                       select 1 from operations.society_post_recipient recipient
                       where recipient.post_id = post.id and recipient.actor_id = ?3
                   )
               )
             order by post.sequence",
        )?;
        let ids = statement
            .query_map(params![space_id, cursor.unwrap_or(0), actor_id], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let posts = ids
            .iter()
            .map(|id| read_board_post(&connection, id, Some(actor_id)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(BoardFeed { cursor, posts })
    }

    pub fn read_board_inbox(&self, actor_id: &str) -> anyhow::Result<Vec<BoardPost>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "with recursive thread(post_id, root_id) as (
                 select id, id from operations.society_post where parent_post_id is null
                 union all
                 select child.id, thread.root_id
                 from operations.society_post child
                 join thread on child.parent_post_id = thread.post_id
             )
             select post.id
             from operations.society_post post
             join thread current_thread on current_thread.post_id = post.id
             join operations.society_post root on root.id = current_thread.root_id
             left join operations.society_attention attention
               on attention.post_id = post.id and attention.recipient_actor_id = ?1
             left join operations.society_post_read post_read
               on post_read.post_id = post.id and post_read.actor_id = ?1
             where post.author_actor_id != ?1
               and post_read.post_id is null
               and (
                   attention.recipient_actor_id is not null
                   or (
                       post.parent_post_id is not null
                       and not exists (
                           select 1 from operations.society_post_recipient recipient
                           where recipient.post_id = post.id
                       )
                       and exists (
                           select 1
                           from operations.society_post participant
                           join thread participant_thread on participant_thread.post_id = participant.id
                           where participant_thread.root_id = current_thread.root_id
                             and participant.author_actor_id = ?1
                             and participant.sequence < post.sequence
                       )
                   )
               )
             order by
               case
                 when attention.recipient_actor_id is not null or root.author_actor_id = ?1 then 0
                 else 1
               end,
               coalesce(attention.amount, 0) desc,
               post.sequence desc",
        )?;
        let ids = statement
            .query_map(params![actor_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        ids.iter()
            .map(|id| read_board_post(&connection, id, Some(actor_id)))
            .collect()
    }

    pub fn record_board_read(
        &self,
        actor_id: &str,
        post_reference: &str,
    ) -> anyhow::Result<BoardReadReceipt> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let post_id = resolve_board_post_id(&transaction, post_reference)?;
        let (sequence, space_id, author_id, recipient_count, is_recipient) = transaction
            .query_row(
                "select post.sequence, post.space_id, post.author_actor_id,
                        (select count(*) from operations.society_post_recipient where post_id = post.id),
                        exists(select 1 from operations.society_post_recipient where post_id = post.id and actor_id = ?2)
                 from operations.society_post post where post.id = ?1",
                params![post_id, actor_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, bool>(4)?,
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("board post not found".to_string()))?;
        if recipient_count > 0 && !is_recipient && author_id != actor_id {
            return Err(StoreError::NotFound("board post not found".to_string()).into());
        }
        let already_read = transaction
            .query_row(
                "select 1 from operations.society_post_read where post_id = ?1 and actor_id = ?2",
                params![post_id, actor_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        let mut settled_obols = 0;
        if !already_read {
            let attention = transaction
                .query_row(
                    "select amount, escrow_id from operations.society_attention
                     where post_id = ?1 and recipient_actor_id = ?2",
                    params![post_id, actor_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            let settlement_ledger_id = if let Some((amount, Some(escrow_id))) = attention {
                let ledger_id = settle_escrow_to_recipient(&transaction, actor_id, &escrow_id)?;
                settled_obols = amount;
                Some(ledger_id)
            } else {
                None
            };
            transaction.execute(
                "insert into operations.society_post_read
                    (post_id, actor_id, read_at, settlement_ledger_id)
                 values (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3)",
                params![post_id, actor_id, settlement_ledger_id],
            )?;
            transaction.execute(
                "insert into operations.society_read_cursor
                    (actor_id, space_id, post_sequence, updated_at)
                 values (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
                 on conflict(actor_id, space_id) do update set
                    post_sequence = max(post_sequence, excluded.post_sequence),
                    updated_at = excluded.updated_at",
                params![actor_id, space_id, sequence],
            )?;
            append_event(
                &transaction,
                Some(actor_id),
                Some(&post_id),
                "board_post_read",
                json!({
                    "spaceId": space_id,
                    "sequence": sequence,
                    "settledObols": settled_obols
                }),
            )?;
        }
        let cursor = transaction
            .query_row(
                "select post_sequence from operations.society_read_cursor
             where actor_id = ?1 and space_id = ?2",
                params![actor_id, space_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        assert_bank_conservation(&transaction)?;
        let post = read_board_post(&transaction, &post_id, Some(actor_id))?;
        transaction.commit()?;
        Ok(BoardReadReceipt {
            post_id,
            post,
            cursor,
            settled_obols,
        })
    }

    pub fn credential_for_username(
        &self,
        username: &str,
    ) -> anyhow::Result<Option<CredentialRecord>> {
        let connection = self.open_workspace_connection()?;
        connection.query_row(
            "select actor_id, username, password_hash from operations.credential where username = ?1 collate nocase",
            params![username.trim()],
            |row| Ok(CredentialRecord { actor_id: row.get(0)?, username: row.get(1)?, password_hash: row.get(2)? }),
        ).optional().map_err(Into::into)
    }

    pub fn create_auth_session(
        &self,
        token: &str,
        actor_id: &str,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "insert into operations.auth_session (token, actor_id, created_at, expires_at)
             values (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?3)",
            params![token, actor_id, expires_at],
        )?;
        Ok(())
    }

    pub fn auth_actor(&self, token: &str, now_epoch: i64) -> anyhow::Result<Option<AuthActor>> {
        let connection = self.open_workspace_connection()?;
        connection
            .query_row(
                "select credential.actor_id, credential.username
             from operations.auth_session
             join operations.credential on credential.actor_id = auth_session.actor_id
             where auth_session.token = ?1 and auth_session.expires_at > ?2",
                params![token, now_epoch],
                |row| {
                    Ok(AuthActor {
                        actor_id: row.get(0)?,
                        username: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn delete_auth_session(&self, token: &str) -> anyhow::Result<()> {
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "delete from operations.auth_session where token = ?1",
            params![token],
        )?;
        Ok(())
    }

    pub fn read_spaces(&self) -> anyhow::Result<Vec<SpaceRef>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select id, settings_json
             from space
             where json_extract(settings_json, '$.title') is not null
               and json_extract(settings_json, '$.society.slug') is null
             order by json_extract(settings_json, '$.order'), id",
        )?;
        let rows = statement.query_map([], |row| {
            let id = row.get::<_, String>(0)?;
            let settings = parse_json(row.get::<_, String>(1)?);
            Ok(SpaceRef {
                id,
                title: string_at(&settings, "/title", "Untitled space"),
                projection: projection_at(&settings, "/projection"),
                neighbors: string_array_at(&settings, "/neighbors"),
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn read_doc(&self, space_id: &str) -> anyhow::Result<SynthesisDoc> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        connection
            .query_row(
                "select space_id, title, version, compiled_at, nodes_json
                 from synthesis_doc
                 where space_id = ?1",
                params![space_id],
                |row| {
                    let nodes_json = row.get::<_, String>(4)?;
                    Ok(SynthesisDoc {
                        space_id: row.get(0)?,
                        title: row.get(1)?,
                        version: row.get(2)?,
                        compiled_at: persisted_timestamp(row.get(3)?),
                        nodes: serde_json::from_str(&nodes_json).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                4,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("synthesis document not found".to_string()).into())
    }

    pub fn read_falsifiers(&self, space_id: &str) -> anyhow::Result<Vec<FalsifierGroup>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let mut statement = connection.prepare(
            "select contract.id, contract.kind, contract.actor_id,
                    coalesce(json_extract(actor.keys_json, '$.displayName'), json_extract(actor.keys_json, '$.username'), contract.actor_id),
                    contract.terms, target.id, target.content, condition.id, condition.content,
                    artifact.id, artifact.content, artifact.metadata_json
             from contract
             join actor on actor.id = contract.actor_id
             join entity target on target.id = contract.target_entity_id
             join entity condition on condition.id = contract.condition_entity_id
             join artifact on artifact.id = contract.evidence_artifact_id
             where contract.kind in ('falsifier', 'conditional_stance')
               and contract.status = 'open' and target.space_id = ?1
               and condition.space_id = target.space_id and condition.role = 'condition'
             order by target.rowid, lower(coalesce(json_extract(actor.keys_json, '$.displayName'), json_extract(actor.keys_json, '$.username'), contract.actor_id)), contract.rowid",
        )?;
        let rows = statement.query_map(params![space_id], |row| {
            let metadata = parse_json(row.get::<_, String>(11)?);
            let content = row.get::<_, String>(10)?;
            Ok((
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                FalsifierReadModel {
                    contract_id: row.get(0)?,
                    kind: contract_kind(&row.get::<_, String>(1)?),
                    actor_id: row.get(2)?,
                    actor_name: row.get(3)?,
                    terms: row.get(4)?,
                    condition_entity_id: row.get(7)?,
                    condition_text: row.get(8)?,
                    evidence: source_link_from_parts(row.get(9)?, content, &metadata),
                },
            ))
        })?;
        let mut groups = Vec::<FalsifierGroup>::new();
        for row in rows {
            let (position_id, position_text, falsifier) = row?;
            if let Some(group) = groups
                .last_mut()
                .filter(|group| group.position_id == position_id)
            {
                group.falsifiers.push(falsifier);
            } else {
                groups.push(FalsifierGroup {
                    position_id,
                    position_text,
                    falsifiers: vec![falsifier],
                });
            }
        }
        Ok(groups)
    }

    pub fn create_space(
        &self,
        actor_id: &str,
        input: CreateSpaceInput,
    ) -> anyhow::Result<SpaceRef> {
        validate_non_empty("title", &input.title)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        ensure_system_actors(&transaction)?;

        let space_id = new_id("space");
        let event_id = new_id("event");
        let next_order: i64 = transaction
            .query_row(
                "select coalesce(max(cast(json_extract(settings_json, '$.order') as integer)), -1) + 1
                 from space",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        transaction.execute(
            "insert into space (id, settings_json)
             values (?1, ?2)",
            params![
                space_id,
                json!({
                    "title": input.title,
                    "projection": input.projection.as_str(),
                    "neighbors": [],
                    "order": next_order,
                })
                .to_string()
            ],
        )?;
        let doc = SynthesisDoc {
            space_id: space_id.clone(),
            title: input.title.clone(),
            version: 1,
            compiled_at: now_label(),
            nodes: vec![],
        };
        write_doc_model(&transaction, &doc)?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'space_created', 'decided', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                actor_id,
                space_id,
                json!({ "projection": input.projection.as_str() }).to_string()
            ],
        )?;
        transaction.commit()?;

        self.read_spaces()?
            .into_iter()
            .find(|space| space.id == space_id)
            .ok_or_else(|| StoreError::NotFound("created space not found".to_string()).into())
    }

    pub fn propose_entity(
        &self,
        actor_id: &str,
        space_id: &str,
        input: CreateEntityInput,
    ) -> anyhow::Result<ReviewItem> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("text", &input.text)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;

        let entity_id = new_id("entity");
        let event_id = new_id("event");
        let review_id = new_id("review");
        let state = initial_entity_state(&input.role);
        let role = input.role.ontology_role();
        let space_title = space_title(&connection, space_id)?;
        let transaction = connection.transaction()?;
        ensure_system_actors(&transaction)?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'entity_create_requested', 'proposed', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                actor_id,
                entity_id,
                json!({
                    "spaceId": space_id,
                    "entityId": entity_id,
                    "role": role,
                    "state": state,
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values
                (?1, ?2, 'entity', ?3, 'pending', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                review_id,
                actor_id,
                event_id,
                json!({
                    "summary": format!("Create {role} in \"{space_title}\""),
                    "detail": input.text,
                    "entity": {
                        "id": entity_id,
                        "spaceId": space_id,
                        "role": role,
                        "state": state,
                        "text": input.text,
                    }
                })
                .to_string()
            ],
        )?;
        transaction.commit()?;

        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .ok_or_else(|| StoreError::NotFound("review item not found".to_string()).into())
    }

    pub fn propose_conditional_stance(
        &self,
        actor_id: &str,
        space_id: &str,
        input: DeclareConditionInput,
    ) -> anyhow::Result<ReviewItem> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("target_entity_id", &input.target_entity_id)?;
        validate_non_empty("terms", &input.terms)?;
        if input.terms.trim() != input.terms || input.terms.chars().count() > 2_000 {
            return Err(StoreError::Validation(
                "condition terms must be trimmed and at most 2000 characters".to_string(),
            )
            .into());
        }
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let target_exists = connection
            .query_row(
                "select 1 from entity where id = ?1 and space_id = ?2 and role = 'issue'",
                params![input.target_entity_id, space_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !target_exists {
            return Err(StoreError::NotFound("target position not found".to_string()).into());
        }
        let owns_stance = connection
            .query_row(
                "select 1 from stance where actor_id = ?1 and target_id = ?2 and axis = 'endorse'
                 order by rowid desc limit 1",
                params![actor_id, input.target_entity_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if !owns_stance {
            return Err(StoreError::Validation(
                "a condition can only be declared on your own stance".to_string(),
            )
            .into());
        }
        let duplicate = connection
            .query_row(
                "select 1 from contract
                 where kind = 'conditional_stance' and actor_id = ?1
                   and target_entity_id = ?2 and terms = ?3",
                params![actor_id, input.target_entity_id, input.terms],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if duplicate {
            return Err(StoreError::Validation(
                "this conditional stance is already declared".to_string(),
            )
            .into());
        }

        let condition_entity_id = new_id("entity");
        let evidence_artifact_id = new_id("artifact");
        let event_id = new_id("event");
        let review_id = new_id("review");
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into artifact (id, kind, media_type, content, metadata_json)
             values (?1, 'source', 'text/plain', ?2, ?3)",
            params![
                evidence_artifact_id,
                input.terms,
                json!({
                    "spaceId": space_id,
                    "inputKind": "conditionalStance",
                    "submittedBy": actor_id,
                    "createdAt": now_label(),
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values (?1, ?2, ?3, 'conditional_stance_requested', 'proposed', ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                actor_id,
                condition_entity_id,
                json!({
                    "spaceId": space_id,
                    "targetEntityId": input.target_entity_id,
                    "conditionEntityId": condition_entity_id,
                    "evidenceArtifactId": evidence_artifact_id,
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values (?1, ?2, 'entity', ?3, 'pending', ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                review_id,
                actor_id,
                event_id,
                json!({
                    "summary": "Declare a condition on your stance",
                    "detail": input.terms,
                    "entity": {
                        "id": condition_entity_id,
                        "spaceId": space_id,
                        "role": "condition",
                        "state": "open",
                        "text": input.terms,
                    },
                    "conditionalStance": {
                        "targetEntityId": input.target_entity_id,
                        "terms": input.terms,
                    }
                })
                .to_string()
            ],
        )?;
        transaction.commit()?;
        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .ok_or_else(|| {
                StoreError::NotFound("condition review item not found".to_string()).into()
            })
    }

    pub fn submit_input(
        &self,
        actor_id: &str,
        space_id: &str,
        input: SubmitInput,
    ) -> anyhow::Result<InputReceipt> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("content", &input.content)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        if input.kind == InputKind::Comment {
            let anchor = input.anchor.as_ref().ok_or_else(|| {
                StoreError::Validation("comment input requires an anchor".to_string())
            })?;
            validate_non_empty("anchor.artifact_id", &anchor.artifact_id)?;
            validate_non_empty("anchor.quote", &anchor.quote)?;
            if anchor.end <= anchor.start {
                return Err(StoreError::Validation(
                    "comment anchor end must be after start".to_string(),
                )
                .into());
            }
            ensure_artifact_contains(&connection, &anchor.artifact_id, &anchor.quote)?;
        } else if input.anchor.is_some() {
            return Err(StoreError::Validation(
                "only comment input may carry an anchor".to_string(),
            )
            .into());
        }

        let artifact_id = new_id("artifact");
        let job_id = new_id("job");
        let artifact_kind = if input.kind == InputKind::Comment {
            "annotation"
        } else {
            "source"
        };
        let transaction = connection.transaction()?;
        ensure_system_actors(&transaction)?;
        transaction.execute(
            "insert into artifact (id, kind, media_type, content, metadata_json)
             values (?1, ?2, 'text/plain', ?3, ?4)",
            params![
                artifact_id,
                artifact_kind,
                input.content,
                json!({
                    "spaceId": space_id,
                    "inputKind": input.kind.as_str(),
                    "anchor": input.anchor,
                    "submittedBy": actor_id,
                    "createdAt": now_label(),
                })
                .to_string()
            ],
        )?;
        if let Some(anchor) = input.anchor.as_ref() {
            transaction.execute(
                "insert into relation
                    (id, from_id, to_id, kind, created_by, confirmed_by)
                 values (?1, ?2, ?3, 'annotates', ?4, ?4)",
                params![
                    new_id("relation"),
                    artifact_id,
                    anchor.artifact_id,
                    actor_id
                ],
            )?;
        }
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'input_submitted', 'decided', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                new_id("event"),
                actor_id,
                artifact_id,
                json!({
                    "spaceId": space_id,
                    "inputKind": input.kind.as_str(),
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.worker_job
                (id, kind, state, payload_json, target_id, created_at, updated_at)
             values
                (?1, 'ingest', 'queued', ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                job_id,
                json!({ "artifactId": artifact_id, "spaceId": space_id }).to_string(),
                artifact_id
            ],
        )?;
        transaction.commit()?;
        Ok(InputReceipt {
            artifact_id,
            job_id,
        })
    }

    pub fn submit_public_objection(
        &self,
        space_id: &str,
        input: PublicObjectionInput,
    ) -> anyhow::Result<ReviewItem> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("report_artifact_id", &input.report_artifact_id)?;
        validate_non_empty("quote", &input.quote)?;
        validate_non_empty("objection", &input.objection)?;
        if input.objection.chars().count() > 4_000 {
            return Err(StoreError::Validation(
                "objection must be at most 4000 characters".to_string(),
            )
            .into());
        }
        if input.quote.chars().count() > 2_000 {
            return Err(StoreError::Validation(
                "selected report span must be at most 2000 characters".to_string(),
            )
            .into());
        }
        if input.report_version < 1 || input.end <= input.start {
            return Err(StoreError::Validation("invalid report anchor range".to_string()).into());
        }
        let display_name = input
            .display_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToString::to_string);
        if display_name
            .as_ref()
            .is_some_and(|name| name.chars().count() > 80)
        {
            return Err(StoreError::Validation(
                "display name must be at most 80 characters".to_string(),
            )
            .into());
        }

        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let (report_content, metadata) = connection
            .query_row(
                "select content, metadata_json from artifact
                 where id = ?1 and kind = 'derived'
                   and json_extract(metadata_json, '$.artifactType') = 'synthesisReport'",
                params![input.report_artifact_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        parse_json(row.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("report artifact not found".to_string()))?;
        if optional_string_at(&metadata, "/spaceId").as_deref() != Some(space_id)
            || metadata.pointer("/reportVersion").and_then(Value::as_i64)
                != Some(input.report_version)
        {
            return Err(StoreError::Validation(
                "objection anchor does not match the named report version".to_string(),
            )
            .into());
        }
        let anchored_text = report_content
            .chars()
            .skip(input.start)
            .take(input.end - input.start)
            .collect::<String>();
        if anchored_text != input.quote || input.quote.chars().count() != input.end - input.start {
            return Err(StoreError::Validation(
                "selected span does not match the named report range".to_string(),
            )
            .into());
        }

        let artifact_id = new_id("artifact");
        let event_id = new_id("event");
        let review_id = new_id("review");
        let anchor = ObjectionAnchor {
            report_artifact_id: input.report_artifact_id.clone(),
            report_version: input.report_version,
            start: input.start,
            end: input.end,
            quote: input.quote,
        };
        let submitter = display_name
            .as_deref()
            .map(|name| format!(" from {name} (unverified)"))
            .unwrap_or_default();
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into artifact (id, kind, media_type, content, metadata_json)
             values (?1, 'annotation', 'text/plain', ?2, ?3)",
            params![
                artifact_id,
                input.objection,
                json!({
                    "spaceId": space_id,
                    "inputKind": "objection",
                    "anchor": anchor,
                    "displayName": display_name,
                    "identity": "unverified",
                    "createdAt": now_label(),
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into relation (id, from_id, to_id, kind, created_by, confirmed_by)
             values (?1, ?2, ?3, 'annotates', null, null)",
            params![new_id("relation"), artifact_id, input.report_artifact_id],
        )?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values (?1, null, ?2, 'objection_submitted', 'proposed', ?3,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                artifact_id,
                json!({ "spaceId": space_id, "sourceArtifactId": artifact_id }).to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values (?1, null, 'objection', ?2, 'pending', ?3,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                review_id,
                event_id,
                json!({
                    "summary": format!("Public objection{submitter} anchored to report v{}", input.report_version),
                    "detail": input.objection,
                    "sourceArtifactId": artifact_id,
                    "unverifiedDisplayName": display_name,
                    "objectionAnchor": anchor,
                })
                .to_string()
            ],
        )?;
        transaction.commit()?;

        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .ok_or_else(|| {
                StoreError::NotFound("objection review item not found".to_string()).into()
            })
    }

    pub fn enqueue_lesswrong_thread_fetch(
        &self,
        actor_id: &str,
        space_id: &str,
        post_id: &str,
    ) -> anyhow::Result<InputReceipt> {
        validate_non_empty("space_id", space_id)?;
        let artifact_id = lesswrong_id("lw-post", post_id)?;
        let target_id = format!("{space_id}:{artifact_id}");
        let candidate_job_id = new_id("job");
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert or ignore into operations.worker_job
                (id, kind, state, payload_json, target_id, created_at, updated_at)
             values (?1, 'lesswrong_thread_fetch', 'queued', ?2, ?3,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                candidate_job_id,
                json!({
                    "artifactId": artifact_id,
                    "postId": post_id,
                    "spaceId": space_id,
                    "requestedBy": actor_id,
                })
                .to_string(),
                target_id,
            ],
        )?;
        let job_id = transaction.query_row(
            "select id from operations.worker_job
             where kind = 'lesswrong_thread_fetch' and target_id = ?1
               and state in ('queued', 'running')
             order by created_at desc, id limit 1",
            params![target_id],
            |row| row.get(0),
        )?;
        transaction.commit()?;
        Ok(InputReceipt {
            artifact_id,
            job_id,
        })
    }

    pub fn import_lesswrong_thread(
        &self,
        space_id: &str,
        thread: LessWrongThreadInput,
    ) -> anyhow::Result<ThreadImportReceipt> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("thread.post_id", &thread.post_id)?;
        validate_non_empty("thread.content", &thread.content)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        validate_authored_timestamp(&connection, "thread.posted_at", &thread.posted_at)?;
        for comment in &thread.comments {
            validate_authored_timestamp(&connection, "comment.posted_at", &comment.posted_at)?;
        }
        let post_artifact_id = lesswrong_id("lw-post", &thread.post_id)?;
        let import_target = format!("{space_id}:{post_artifact_id}");
        let mut source_artifact_ids = vec![post_artifact_id.clone()];
        source_artifact_ids.extend(
            thread
                .comments
                .iter()
                .map(|comment| lesswrong_id("lw-comment", &comment.external_id))
                .collect::<anyhow::Result<Vec<_>>>()?,
        );
        let previously_seen = {
            let mut statement = connection.prepare(
                "select payload_json from operations.worker_job
                 where kind = 'lesswrong_thread_ingest'
                   and state in ('queued', 'running', 'succeeded')
                   and json_extract(payload_json, '$.artifactId') = ?1
                   and json_extract(payload_json, '$.spaceId') = ?2",
            )?;
            let rows = statement.query_map(params![post_artifact_id, space_id], |row| {
                Ok(parse_json(row.get::<_, String>(0)?))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .flat_map(|payload| {
                    payload
                        .get("newSourceArtifactIds")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                })
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<BTreeSet<_>>()
        };
        let new_source_artifact_ids = source_artifact_ids
            .iter()
            .filter(|id| !previously_seen.contains(*id))
            .cloned()
            .collect::<Vec<_>>();
        let job_ids = new_source_artifact_ids
            .chunks(THREAD_MERGE_BATCH_SIZE)
            .map(|_| new_id("job"))
            .collect::<Vec<_>>();
        let transaction = connection.transaction()?;
        ensure_system_actors(&transaction)?;
        upsert_lesswrong_actor(&transaction, &thread.author)?;
        for comment in &thread.comments {
            upsert_lesswrong_actor(&transaction, &comment.author)?;
        }
        transaction.execute(
            "insert into artifact (id, kind, media_type, content, metadata_json)
             values (?1, 'source', 'text/plain', ?2, ?3)
             on conflict(id) do nothing",
            params![
                post_artifact_id,
                thread.content,
                json!({
                    "spaceId": space_id,
                    "inputKind": "lesswrongThread",
                    "sourceKind": "post",
                    "externalId": thread.post_id,
                    "threadArtifactId": post_artifact_id,
                    "title": thread.title,
                    "sourceUrl": thread.url,
                    "authorId": lesswrong_id("lw-actor", &thread.author.external_id)?,
                    "postedAt": thread.posted_at,
                })
                .to_string()
            ],
        )?;
        for comment in &thread.comments {
            let artifact_id = lesswrong_id("lw-comment", &comment.external_id)?;
            let parent_artifact_id = comment
                .parent_external_id
                .as_deref()
                .map(|id| lesswrong_id("lw-comment", id))
                .transpose()?
                .unwrap_or_else(|| post_artifact_id.clone());
            transaction.execute(
                "insert into artifact (id, kind, media_type, content, metadata_json)
                 values (?1, 'source', 'text/plain', ?2, ?3)
                 on conflict(id) do nothing",
                params![
                    artifact_id,
                    comment.content,
                    json!({
                        "spaceId": space_id,
                        "inputKind": "lesswrongThread",
                        "sourceKind": "comment",
                        "externalId": comment.external_id,
                        "threadArtifactId": post_artifact_id,
                        "sourceUrl": format!("{}?commentId={}", thread.url, comment.external_id),
                        "authorId": lesswrong_id("lw-actor", &comment.author.external_id)?,
                        "parentArtifactId": parent_artifact_id,
                        "postedAt": comment.posted_at,
                    })
                    .to_string()
                ],
            )?;
        }
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values (?1, ?2, ?3, 'lesswrong_thread_imported', 'decided', ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                new_id("event"),
                LOCAL_ACTOR_ID,
                post_artifact_id,
                json!({
                    "spaceId": space_id,
                    "sourceUrl": thread.url,
                    "newSourceArtifactIds": new_source_artifact_ids,
                })
                .to_string()
            ],
        )?;
        for (job_id, batch) in job_ids
            .iter()
            .zip(new_source_artifact_ids.chunks(THREAD_MERGE_BATCH_SIZE))
        {
            transaction.execute(
                "insert into operations.worker_job
                    (id, kind, state, payload_json, target_id, created_at, updated_at)
                 values (?1, 'lesswrong_thread_ingest', 'queued', ?2, ?3,
                         strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![
                    job_id,
                    json!({
                        "artifactId": post_artifact_id,
                        "spaceId": space_id,
                        "newSourceArtifactIds": batch,
                    }).to_string(),
                    import_target
                ],
            )?;
        }
        transaction.commit()?;
        Ok(ThreadImportReceipt {
            artifact_id: post_artifact_id,
            ingest_job_id: job_ids.first().cloned(),
            new_source_count: new_source_artifact_ids.len(),
        })
    }

    pub fn read_artifact(&self, artifact_id: &str) -> anyhow::Result<ArtifactReadModel> {
        validate_non_empty("artifact_id", artifact_id)?;
        let connection = self.open_workspace_connection()?;
        connection
            .query_row(
                "select kind, media_type, coalesce(content, ''), metadata_json
             from artifact where id = ?1 and kind != 'board_post'",
                params![artifact_id],
                |row| {
                    let metadata = parse_json(row.get::<_, String>(3)?);
                    Ok(ArtifactReadModel {
                        artifact_id: artifact_id.to_string(),
                        kind: row.get(0)?,
                        media_type: row.get(1)?,
                        content: row.get(2)?,
                        source_url: optional_string_at(&metadata, "/sourceUrl"),
                    })
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("artifact not found".to_string()).into())
    }

    pub fn read_imported_sources(&self, space_id: &str) -> anyhow::Result<Vec<ImportedSource>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select distinct target_id from events.event
             where kind = 'lesswrong_thread_imported'
               and json_extract(payload_json, '$.spaceId') = ?1
             order by rowid",
        )?;
        let roots = statement
            .query_map(params![space_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut sources = Vec::new();
        let mut seen = BTreeSet::new();
        for root in roots {
            for source in self.read_imported_thread_sources(&root)? {
                if seen.insert(source.artifact_id.clone()) {
                    sources.push(source);
                }
            }
        }
        Ok(sources)
    }

    pub fn read_imported_thread_sources(
        &self,
        post_artifact_id: &str,
    ) -> anyhow::Result<Vec<ImportedSource>> {
        validate_non_empty("post_artifact_id", post_artifact_id)?;
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select artifact.id, artifact.content, artifact.metadata_json,
                    coalesce(json_extract(actor.keys_json, '$.displayName'), 'Unknown')
             from artifact
             left join actor on actor.id = json_extract(artifact.metadata_json, '$.authorId')
             where json_extract(artifact.metadata_json, '$.threadArtifactId') = ?1
               and json_extract(artifact.metadata_json, '$.inputKind') = 'lesswrongThread'
             order by case json_extract(artifact.metadata_json, '$.sourceKind') when 'post' then 0 else 1 end,
                      json_extract(artifact.metadata_json, '$.postedAt'), artifact.id",
        )?;
        let rows = statement.query_map(params![post_artifact_id], |row| {
            let metadata = parse_json(row.get::<_, String>(2)?);
            Ok(ImportedSource {
                artifact_id: row.get(0)?,
                thread_artifact_id: string_at(&metadata, "/threadArtifactId", post_artifact_id),
                source_kind: string_at(&metadata, "/sourceKind", "comment"),
                author_id: string_at(&metadata, "/authorId", ""),
                author_name: row.get(3)?,
                content: row.get(1)?,
                source_url: string_at(&metadata, "/sourceUrl", ""),
                posted_at: string_at(&metadata, "/postedAt", ""),
                parent_artifact_id: optional_string_at(&metadata, "/parentArtifactId"),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn build_thread_ingest_context(
        &self,
        artifact_id: &str,
        space_id: &str,
        new_source_artifact_ids: &[String],
        standing_limit: usize,
        new_source_limit: usize,
    ) -> anyhow::Result<ThreadIngestContext> {
        validate_non_empty("artifact_id", artifact_id)?;
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        let metadata = connection
            .query_row(
                "select metadata_json from artifact where id = ?1
             and json_extract(metadata_json, '$.sourceKind') = 'post'",
                params![artifact_id],
                |row| Ok(parse_json(row.get::<_, String>(0)?)),
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("LessWrong post artifact not found".to_string()))?;
        if new_source_artifact_ids.len() > new_source_limit {
            return Err(StoreError::Validation(
                "merge context exceeds its declared source bound".to_string(),
            )
            .into());
        }
        let requested = new_source_artifact_ids.iter().collect::<BTreeSet<_>>();
        let new_sources = self
            .read_imported_thread_sources(artifact_id)?
            .into_iter()
            .filter(|source| requested.contains(&source.artifact_id))
            .collect::<Vec<_>>();
        if new_sources.len() != new_source_artifact_ids.len() {
            return Err(StoreError::Validation(
                "merge context contains unknown new source artifacts".to_string(),
            )
            .into());
        }
        let mut statement = connection.prepare(
            "select id, content from entity
             where space_id = ?1 and role = 'issue'
             order by rowid limit ?2",
        )?;
        let standing_positions = statement
            .query_map(params![space_id, standing_limit as i64], |row| {
                Ok(StandingPosition {
                    entity_id: row.get(0)?,
                    text: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ThreadIngestContext {
            space_title: space_title(&connection, space_id)?,
            standing_positions,
            new_sources,
            space_id: space_id.to_string(),
            thread_url: string_at(&metadata, "/sourceUrl", ""),
            title: string_at(&metadata, "/title", "LessWrong thread"),
        })
    }

    pub fn record_thread_ingest_proposal(
        &self,
        context: ThreadIngestContext,
        positions: Vec<ThreadPositionProposal>,
    ) -> anyhow::Result<Option<ReviewItem>> {
        validate_thread_position_proposals(&context, &positions)?;
        if positions.is_empty() {
            return Ok(None);
        }
        let mut connection = self.open_workspace_connection()?;
        let event_id = new_id("event");
        let review_id = new_id("review");
        let artifact_id = context
            .new_sources
            .first()
            .map(|source| source.thread_artifact_id.clone())
            .ok_or_else(|| {
                StoreError::Validation("landscape delta has no new sources".to_string())
            })?;
        let findings = positions
            .into_iter()
            .map(|position| {
                json!({
                    "id": (position.action == "new").then(|| new_id("entity")),
                    "action": position.action,
                    "targetText": position.target_entity_id.as_ref().and_then(|target_id| {
                        context.standing_positions.iter()
                            .find(|standing| &standing.entity_id == target_id)
                            .map(|standing| standing.text.clone())
                    }),
                    "targetEntityId": position.target_entity_id,
                    "key": position.key,
                    "text": position.text,
                    "reasoning": position.reasoning,
                    "sourceArtifactIds": position.source_artifact_ids,
                    "stances": position.stances,
                })
            })
            .collect::<Vec<_>>();
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values (?1, ?2, ?3, 'lesswrong_landscape_proposed', 'proposed', ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                AGENT_ACTOR_ID,
                artifact_id,
                json!({
                    "spaceId": context.space_id,
                    "sourceArtifactId": artifact_id,
                    "findingCount": findings.len(),
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values (?1, ?2, 'ingest', ?3, 'pending', ?4,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![review_id, LOCAL_ACTOR_ID, event_id, json!({
                "summary": format!("Review {} position landscape delta finding(s)", findings.len()),
                "detail": findings.iter().map(|finding| {
                    let action = string_at(finding, "/action", "new");
                    let text = if action == "attach" {
                        string_at(finding, "/targetText", "")
                    } else {
                        string_at(finding, "/text", "")
                    };
                    let reasoning = string_at(finding, "/reasoning", "");
                    let stances = finding
                        .get("stances")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|stance| {
                            let actor = string_at(stance, "/actorId", "unknown actor");
                            let value = string_at(stance, "/value", "unknown stance");
                            let evidence = string_at(stance, "/evidenceArtifactId", "no evidence");
                            let confidence = optional_string_at(stance, "/statedConfidence")
                                .map(|value| format!(", stated confidence “{value}”"))
                                .unwrap_or_default();
                            format!("{actor} {value}; evidence {evidence}{confidence}")
                        })
                        .collect::<Vec<_>>();
                    let stance_detail = if stances.is_empty() {
                        String::new()
                    } else {
                        format!(" — {}", stances.join("; "))
                    };
                    format!("{action}: {text} — {reasoning}{stance_detail}")
                }).collect::<Vec<_>>().join(" · "),
                "entity": { "sourceArtifactId": artifact_id },
                "findings": findings,
            }).to_string()],
        )?;
        transaction.commit()?;
        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .map(Some)
            .ok_or_else(|| {
                StoreError::NotFound("thread ingest review item not found".to_string()).into()
            })
    }

    pub fn build_ingest_context(
        &self,
        artifact_id: &str,
        target_excerpt_radius_chars: usize,
    ) -> anyhow::Result<IngestContext> {
        validate_non_empty("artifact_id", artifact_id)?;
        let connection = self.open_workspace_connection()?;
        let (content, metadata) = connection
            .query_row(
                "select content, metadata_json
                 from artifact
                 where id = ?1 and kind in ('source', 'annotation')",
                params![artifact_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        parse_json(row.get::<_, String>(1)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("raw artifact not found".to_string()))?;
        let space_id = string_at(&metadata, "/spaceId", "");
        validate_non_empty("artifact space_id", &space_id)?;
        let input_kind = match string_at(&metadata, "/inputKind", "text").as_str() {
            "document" => InputKind::Document,
            "comment" => InputKind::Comment,
            _ => InputKind::Text,
        };
        let anchor: Option<InputAnchor> = metadata
            .get("anchor")
            .cloned()
            .filter(|value| !value.is_null())
            .map(serde_json::from_value)
            .transpose()?;
        let target_excerpt = anchor
            .as_ref()
            .map(|anchor| {
                connection.query_row(
                    "select content from artifact where id = ?1",
                    params![anchor.artifact_id],
                    |row| row.get::<_, String>(0),
                )
            })
            .transpose()?
            .and_then(|target| {
                excerpt_around_quote(
                    &target,
                    anchor.as_ref()?.quote.as_str(),
                    target_excerpt_radius_chars,
                )
            });
        Ok(IngestContext {
            artifact_id: artifact_id.to_string(),
            space_title: space_title(&connection, &space_id)?,
            space_id,
            input_kind,
            content,
            anchor,
            target_excerpt,
        })
    }

    pub fn record_ingest_proposal(
        &self,
        artifact_id: &str,
        input: CreateEntityInput,
    ) -> anyhow::Result<ReviewItem> {
        validate_non_empty("artifact_id", artifact_id)?;
        validate_non_empty("text", &input.text)?;
        let context = self.build_ingest_context(artifact_id, 1_200)?;
        let mut connection = self.open_workspace_connection()?;
        let entity_id = new_id("entity");
        let event_id = new_id("event");
        let review_id = new_id("review");
        let role = input.role.ontology_role();
        let state = initial_entity_state(&input.role);
        let transaction = connection.transaction()?;
        ensure_system_actors(&transaction)?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'ingest_entity_proposed', 'proposed', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                event_id,
                AGENT_ACTOR_ID,
                entity_id,
                json!({
                    "spaceId": context.space_id,
                    "entityId": entity_id,
                    "role": role,
                    "state": state,
                    "sourceArtifactId": artifact_id,
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values
                (?1, ?2, 'ingest', ?3, 'pending', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                review_id,
                LOCAL_ACTOR_ID,
                event_id,
                json!({
                    "summary": format!("Integrate {role} from {}", context.input_kind.as_str()),
                    "detail": input.text,
                    "entity": {
                        "id": entity_id,
                        "spaceId": context.space_id,
                        "role": role,
                        "state": state,
                        "text": input.text,
                        "sourceArtifactId": artifact_id,
                    }
                })
                .to_string()
            ],
        )?;
        transaction.commit()?;
        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .ok_or_else(|| StoreError::NotFound("ingest review item not found".to_string()).into())
    }

    pub fn enqueue_synthesis(
        &self,
        actor_id: &str,
        space_id: &str,
        input: SynthesizeInput,
    ) -> anyhow::Result<WorkerJobReceipt> {
        validate_non_empty("space_id", space_id)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let mode = input.mode();
        if mode == SynthesisMode::Selective {
            validate_non_empty("instruction", input.instruction().unwrap_or_default())?;
            if self.read_report(space_id)?.is_none() {
                return Err(StoreError::Validation(
                    "selective synthesis requires an existing report".to_string(),
                )
                .into());
            }
        }
        let job_id = new_id("job");
        let transaction = connection.transaction()?;
        transaction.execute(
            "delete from operations.worker_job
             where kind = 'synthesize' and state = 'queued' and target_id = ?1
               and json_extract(payload_json, '$.auto') = 1",
            params![space_id],
        )?;
        transaction.execute(
            "insert into operations.worker_job
                (id, kind, state, payload_json, target_id, created_at, updated_at)
             values
                (?1, 'synthesize', 'queued', ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                job_id,
                json!({
                    "spaceId": space_id,
                    "mode": mode.as_str(),
                    "instruction": input.instruction(),
                    "requestedBy": actor_id,
                })
                .to_string(),
                space_id
            ],
        )?;
        transaction.commit()?;
        Ok(WorkerJobReceipt { job_id })
    }

    pub fn enqueue_auto_synthesis(
        &self,
        actor_id: &str,
        space_id: &str,
    ) -> anyhow::Result<WorkerJobReceipt> {
        validate_non_empty("space_id", space_id)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let transaction = connection.transaction()?;
        if let Some(job_id) = transaction
            .query_row(
                "select id from operations.worker_job
                 where kind = 'synthesize' and state = 'queued' and target_id = ?1
                   and json_extract(payload_json, '$.auto') = 1",
                params![space_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
        {
            transaction.execute(
                "update operations.worker_job
                 set updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     payload_json = json_set(payload_json, '$.requestedBy', ?2)
                 where id = ?1",
                params![job_id, actor_id],
            )?;
            transaction.commit()?;
            return Ok(WorkerJobReceipt { job_id });
        }
        let job_id = new_id("job");
        transaction.execute(
            "insert into operations.worker_job
                (id, kind, state, payload_json, target_id, created_at, updated_at)
             values (?1, 'synthesize', 'queued', ?2, ?3,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                job_id,
                json!({
                    "spaceId": space_id,
                    "mode": "full",
                    "instruction": null,
                    "requestedBy": actor_id,
                    "auto": true,
                })
                .to_string(),
                space_id,
            ],
        )?;
        transaction.commit()?;
        Ok(WorkerJobReceipt { job_id })
    }

    pub fn record_synthesis_routing(
        &self,
        job_id: &str,
        mode: SynthesisMode,
        reason: &str,
    ) -> anyhow::Result<()> {
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_job
             set payload_json = json_set(payload_json,
                 '$.routedMode', ?2, '$.routingReason', ?3)
             where id = ?1",
            params![job_id, mode.as_str(), reason],
        )?;
        Ok(())
    }

    pub fn read_report(&self, space_id: &str) -> anyhow::Result<Option<SynthesisReport>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        read_report_model(&connection, space_id)
    }

    pub fn read_report_history(&self, space_id: &str) -> anyhow::Result<Vec<ReportVersion>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        read_report_history_model(&connection, space_id)
    }

    pub fn read_report_version_diff(
        &self,
        space_id: &str,
        version: i64,
    ) -> anyhow::Result<ReportVersionDiff> {
        validate_non_empty("space_id", space_id)?;
        if version < 1 {
            return Err(
                StoreError::Validation("report version must be positive".to_string()).into(),
            );
        }
        let connection = self.open_workspace_connection()?;
        let selected = read_report_history_model(&connection, space_id)?
            .into_iter()
            .find(|candidate| candidate.report.version == version)
            .ok_or_else(|| StoreError::NotFound("report version not found".to_string()))?;
        let hunks = diff_hunks(&selected.diff_from_previous)
            .into_iter()
            .map(|lines| ReportDiffHunk {
                lines,
                caused_by: selected.caused_by.clone(),
            })
            .collect();
        Ok(ReportVersionDiff {
            version,
            previous_version: (version > 1).then_some(version - 1),
            hunks,
        })
    }

    pub fn read_feed(&self, space_id: &str, after: Option<&str>) -> anyhow::Result<Vec<FeedEntry>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        let after = after.filter(|value| !value.is_empty());
        if let Some(event_id) = after {
            let exists = connection.query_row(
                "select exists(
                    select 1 from events.event event
                    where event.id = ?1
                      and json_extract(event.payload_json, '$.spaceId') = ?2
                      and ((event.kind in ('report_written', 'synthesis_report_written')
                            and event.decision_state = 'decided')
                           or (event.kind = 'objection_submitted'
                               and event.decision_state in ('decided', 'rejected'))
                           or (event.kind in ('lesswrong_landscape_delta_accepted', 'ingest_entity_proposed',
                                              'entity_create_requested', 'session_commit',
                                              'conditional_stance_requested', 'entity_text_updated',
                                              'entity_state_changed')
                               and event.decision_state = 'decided'))
                )",
                params![event_id, space_id],
                |row| row.get::<_, bool>(0),
            )?;
            if !exists {
                return Err(StoreError::NotFound("feed cursor not found".to_string()).into());
            }
        }
        let mut statement = connection.prepare(
            "with candidates as (
                select event.rowid as event_rowid, event.id, event.target_id, event.kind,
                       event.decision_state, event.payload_json,
                       coalesce(review.resolved_at, event.created_at) as effective_at
                from events.event event
                left join operations.attention_item review
                  on review.target_id = event.id and review.state = 'resolved'
                where json_extract(event.payload_json, '$.spaceId') = ?1
                  and (
                    (event.kind in ('report_written', 'synthesis_report_written')
                     and event.decision_state = 'decided')
                    or (event.kind = 'objection_submitted'
                        and event.decision_state in ('decided', 'rejected'))
                    or (event.kind in ('lesswrong_landscape_delta_accepted', 'ingest_entity_proposed',
                                       'entity_create_requested', 'session_commit',
                                       'conditional_stance_requested', 'entity_text_updated',
                                       'entity_state_changed')
                        and event.decision_state = 'decided')
                  )
            ), cursor as (
                select effective_at, event_rowid from candidates where id = ?2
            )
            select event_rowid, id, target_id, kind, decision_state, payload_json, effective_at
            from candidates
            where ?2 is null
               or effective_at > (select effective_at from cursor)
               or (effective_at = (select effective_at from cursor)
                   and event_rowid > (select event_rowid from cursor))
            order by effective_at, event_rowid
            limit 100",
        )?;
        let rows = statement.query_map(params![space_id, after], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                parse_json(row.get::<_, String>(5)?),
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (_rowid, event_id, target_id, event_kind, decision_state, payload, created_at) =
                row?;
            if matches!(
                event_kind.as_str(),
                "report_written" | "synthesis_report_written"
            ) {
                let causes = connection
                    .query_row(
                        "select metadata_json from artifact where id = ?1",
                        params![target_id],
                        |row| Ok(parse_json(row.get::<_, String>(0)?)),
                    )?
                    .get("causedBy")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                let version = payload.pointer("/version").and_then(Value::as_i64);
                return Ok(FeedEntry {
                    event_id,
                    kind: FeedEntryKind::VersionWritten,
                    label: version.map_or_else(
                        || "Report version written".to_string(),
                        |v| format!("Report version {v} written"),
                    ),
                    created_at,
                    report_version: version,
                    report_artifact_id: Some(target_id),
                    causes,
                    cause: None,
                    objection_outcome: None,
                });
            }
            let mut cause =
                read_feed_cause(&connection, &event_id, &event_kind, &target_id, &payload)?;
            if event_kind == "objection_submitted" {
                let outcome = if decision_state == "decided" {
                    ObjectionOutcome::Accepted
                } else {
                    cause.label = "Rejected a public objection".to_string();
                    ObjectionOutcome::Rejected
                };
                return Ok(FeedEntry {
                    event_id,
                    kind: FeedEntryKind::ObjectionResolved,
                    label: cause.label.clone(),
                    created_at,
                    report_version: cause
                        .objection_anchor
                        .as_ref()
                        .map(|anchor| anchor.report_version),
                    report_artifact_id: cause
                        .objection_anchor
                        .as_ref()
                        .map(|anchor| anchor.report_artifact_id.clone()),
                    causes: vec![],
                    cause: Some(cause),
                    objection_outcome: Some(outcome),
                });
            }
            Ok(FeedEntry {
                event_id,
                kind: FeedEntryKind::CauseAccepted,
                label: cause.label.clone(),
                created_at,
                report_version: None,
                report_artifact_id: None,
                causes: vec![],
                cause: Some(cause),
                objection_outcome: None,
            })
        })
        .collect()
    }

    pub fn put_refresh_schedule(
        &self,
        space_id: &str,
        input: PutRefreshScheduleInput,
    ) -> anyhow::Result<RefreshSchedule> {
        validate_non_empty("space_id", space_id)?;
        if !(1..=31_536_000).contains(&input.cadence_seconds) {
            return Err(StoreError::Validation(
                "cadence_seconds must be between 1 and 31536000".to_string(),
            )
            .into());
        }
        let connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        connection.execute(
            "insert into operations.refresh_schedule
                (space_id, cadence_seconds, next_due_at, enabled, created_at)
             values (?1, ?2,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now', printf('+%d seconds', ?2)),
                     ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             on conflict(space_id) do update set
                cadence_seconds = excluded.cadence_seconds,
                next_due_at = excluded.next_due_at,
                enabled = excluded.enabled",
            params![space_id, input.cadence_seconds, input.enabled],
        )?;
        read_refresh_schedule_model(&connection, space_id)
    }

    pub fn read_refresh_schedule(&self, space_id: &str) -> anyhow::Result<Option<RefreshSchedule>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        read_optional_refresh_schedule_model(&connection, space_id)
    }

    pub fn scan_due_refresh_schedules(&self) -> anyhow::Result<usize> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let due = {
            let mut statement = transaction.prepare(
                "select space_id, cadence_seconds, next_due_at
                 from operations.refresh_schedule
                 where enabled = 1 and julianday(next_due_at) <= julianday('now')
                 order by next_due_at, space_id",
            )?;
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?
        };
        for (space_id, cadence_seconds, previous_due) in &due {
            let roots = {
                let mut statement = transaction.prepare(
                    "select distinct json_extract(artifact.metadata_json, '$.externalId')
                     from artifact
                     where json_extract(artifact.metadata_json, '$.spaceId') = ?1
                       and json_extract(artifact.metadata_json, '$.inputKind') = 'lesswrongThread'
                       and json_extract(artifact.metadata_json, '$.sourceKind') = 'post'
                     order by artifact.rowid",
                )?;
                statement
                    .query_map(params![space_id], |row| row.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for post_id in roots {
                let artifact_id = lesswrong_id("lw-post", &post_id)?;
                let target_id = format!("{space_id}:{artifact_id}");
                transaction.execute(
                    "insert or ignore into operations.worker_job
                        (id, kind, state, payload_json, target_id, created_at, updated_at)
                     values (?1, 'lesswrong_thread_fetch', 'queued', ?2, ?3,
                             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                    params![
                        new_id("job"),
                        json!({
                            "artifactId": artifact_id,
                            "postId": post_id,
                            "spaceId": space_id,
                            "requestedBy": AGENT_ACTOR_ID,
                            "scheduled": true,
                        })
                        .to_string(),
                        target_id,
                    ],
                )?;
            }
            transaction.execute(
                "update operations.refresh_schedule
                 set next_due_at = strftime('%Y-%m-%dT%H:%M:%fZ', ?2,
                                            printf('+%d seconds', ?3))
                 where space_id = ?1 and enabled = 1 and next_due_at = ?2",
                params![space_id, previous_due, cadence_seconds],
            )?;
        }
        transaction.commit()?;
        Ok(due.len())
    }

    pub fn build_synthesis_context(&self, space_id: &str) -> anyhow::Result<SynthesisContext> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        let doc = read_doc_model(&connection, space_id)?;
        let mut statement = connection.prepare(
            "select distinct artifact.id, artifact.content, artifact.metadata_json,
                    coalesce(json_extract(actor.keys_json, '$.username'),
                             json_extract(actor.keys_json, '$.displayName'))
             from entity
             join relation on relation.from_id = entity.id and relation.kind = 'derived_from'
             join artifact on artifact.id = relation.to_id
             left join actor on actor.id = coalesce(
                 json_extract(artifact.metadata_json, '$.authorId'),
                 json_extract(artifact.metadata_json, '$.submittedBy'))
             where entity.space_id = ?1 and artifact.kind in ('source', 'annotation')
             order by artifact.rowid, artifact.id",
        )?;
        let rows = statement.query_map(params![space_id], |row| {
            let metadata = parse_json(row.get::<_, String>(2)?);
            Ok(SynthesisSource {
                artifact_id: row.get(0)?,
                content: row.get(1)?,
                input_kind: string_at(&metadata, "/inputKind", "text"),
                author_name: row.get(3)?,
                source_url: optional_string_at(&metadata, "/sourceUrl"),
            })
        })?;
        let sources = rows.collect::<Result<Vec<_>, _>>()?;
        let causes = report_causes_since_previous(&connection, space_id)?;
        Ok(SynthesisContext {
            space_id: space_id.to_string(),
            title: doc.title.clone(),
            compiled_document: doc,
            sources,
            causes,
            current_report: read_report_model(&connection, space_id)?,
        })
    }

    pub fn committed_synthesis_effect(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<SynthesisReport>> {
        validate_non_empty("job_id", job_id)?;
        let connection = self.open_workspace_connection()?;
        let artifact_id = connection
            .query_row(
                "select effect_artifact_id from operations.worker_job where id = ?1",
                params![job_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        artifact_id
            .map(|artifact_id| read_report_artifact(&connection, &artifact_id))
            .transpose()
    }

    pub fn write_synthesis_report(
        &self,
        synthesis_job_id: Option<&str>,
        space_id: &str,
        mode: SynthesisMode,
        routing_reason: Option<&str>,
        content: &str,
        causes: &[ReportCause],
    ) -> anyhow::Result<SynthesisReport> {
        self.write_report(
            synthesis_job_id,
            AGENT_ACTOR_ID,
            "synthesis_report_written",
            space_id,
            mode,
            routing_reason,
            content,
            Some(causes),
        )
    }

    pub fn write_authored_report(
        &self,
        actor_id: &str,
        space_id: &str,
        input: WriteReportInput,
    ) -> anyhow::Result<SynthesisReport> {
        validate_non_empty("actor_id", actor_id)?;
        validate_non_empty("content", &input.content)?;
        self.write_report(
            None,
            actor_id,
            "report_written",
            space_id,
            SynthesisMode::Full,
            Some("human-authored report version"),
            &input.content,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn write_report(
        &self,
        synthesis_job_id: Option<&str>,
        actor_id: &str,
        event_kind: &str,
        space_id: &str,
        mode: SynthesisMode,
        routing_reason: Option<&str>,
        content: &str,
        causes: Option<&[ReportCause]>,
    ) -> anyhow::Result<SynthesisReport> {
        validate_non_empty("space_id", space_id)?;
        validate_non_empty("content", content)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_space_exists(&connection, space_id)?;
        if let Some(job_id) = synthesis_job_id {
            if let Some(report) = self.committed_synthesis_effect(job_id)? {
                return Ok(report);
            }
        }
        let transaction = connection.transaction()?;
        let previous = read_report_model(&transaction, space_id)?;
        let computed_causes = causes
            .is_none()
            .then(|| report_causes_since_previous(&transaction, space_id))
            .transpose()?;
        let causes = causes.unwrap_or_else(|| {
            computed_causes
                .as_deref()
                .expect("missing causes are computed in the report transaction")
        });
        if mode == SynthesisMode::Selective && previous.is_none() {
            return Err(StoreError::Validation(
                "selective synthesis requires an existing report".to_string(),
            )
            .into());
        }
        let provenance_anchors = derive_report_provenance(&transaction, space_id, content)?;
        let artifact_id = new_id("artifact");
        let version = previous
            .as_ref()
            .map(|report| report.version + 1)
            .unwrap_or(1);
        let created_at = now_label();
        ensure_system_actors(&transaction)?;
        transaction.execute(
            "insert into artifact
                (id, kind, media_type, content, derived_from, metadata_json)
             values
                (?1, 'derived', 'text/plain', ?2, ?3, ?4)",
            params![
                artifact_id,
                content,
                previous.as_ref().map(|report| report.artifact_id.as_str()),
                json!({
                    "artifactType": "synthesisReport",
                    "spaceId": space_id,
                    "synthesisJobId": synthesis_job_id,
                    "reportVersion": version,
                    "mode": mode.as_str(),
                    "routingReason": routing_reason,
                    "createdAt": created_at,
                    "causedBy": causes,
                    "provenanceAnchors": &provenance_anchors,
                })
                .to_string()
            ],
        )?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, ?4, 'decided', ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                new_id("event"),
                actor_id,
                artifact_id,
                event_kind,
                json!({
                    "spaceId": space_id,
                    "version": version,
                    "mode": mode.as_str(),
                    "synthesisJobId": synthesis_job_id,
                    "previousArtifactId": previous.as_ref().map(|report| &report.artifact_id),
                })
                .to_string()
            ],
        )?;
        if let Some(job_id) = synthesis_job_id {
            let changed = transaction.execute(
                "update operations.worker_job
                 set effect_artifact_id = ?2,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 where id = ?1 and effect_artifact_id is null",
                params![job_id, artifact_id],
            )?;
            if changed != 1 {
                return Err(StoreError::Validation(
                    "synthesis job effect was already committed".to_string(),
                )
                .into());
            }
        }
        transaction.commit()?;
        Ok(SynthesisReport {
            artifact_id,
            space_id: space_id.to_string(),
            version,
            content: content.to_string(),
            mode,
            routing_reason: routing_reason.map(ToString::to_string),
            created_at,
            provenance_anchors,
        })
    }

    pub fn edit_entity_text(
        &self,
        actor_id: &str,
        entity_id: &str,
        input: EditEntityTextInput,
    ) -> anyhow::Result<SynthesisDoc> {
        validate_non_empty("entity_id", entity_id)?;
        validate_non_empty("text", &input.text)?;
        let mut connection = self.open_workspace_connection()?;
        let (space_id, _) = entity_space_and_role(&connection, entity_id)?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "update entity
             set content = ?2
             where id = ?1",
            params![entity_id, input.text],
        )?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'entity_text_updated', 'decided', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                new_id("event"),
                actor_id,
                entity_id,
                json!({ "spaceId": space_id }).to_string()
            ],
        )?;
        let doc = recompile_doc_projection(&transaction, &space_id)?;
        transaction.commit()?;
        Ok(doc)
    }

    pub fn change_entity_state(
        &self,
        actor_id: &str,
        entity_id: &str,
        input: ChangeEntityStateInput,
    ) -> anyhow::Result<SynthesisDoc> {
        validate_non_empty("entity_id", entity_id)?;
        validate_non_empty("state", &input.state)?;
        let mut connection = self.open_workspace_connection()?;
        let (space_id, role) = entity_space_and_role(&connection, entity_id)?;
        validate_entity_state(&role, &input.state)?;
        let current_state = connection.query_row(
            "select state from entity where id = ?1",
            params![entity_id],
            |row| row.get::<_, String>(0),
        )?;
        let next_state = state_with_label(&current_state, &input.state);
        let transaction = connection.transaction()?;
        transaction.execute(
            "update entity
             set state = ?2
             where id = ?1",
            params![entity_id, next_state],
        )?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'entity_state_changed', 'decided', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                new_id("event"),
                actor_id,
                entity_id,
                json!({
                    "spaceId": space_id,
                    "oldState": current_state,
                    "newState": next_state,
                })
                .to_string()
            ],
        )?;
        let doc = recompile_doc_projection(&transaction, &space_id)?;
        transaction.commit()?;
        Ok(doc)
    }

    pub fn read_review(&self) -> anyhow::Result<Vec<ReviewItem>> {
        let connection = self.open_workspace_connection()?;
        let mut statement = connection.prepare(
            "select id, kind, payload_json
             from operations.attention_item
             where state = 'pending'
             order by created_at desc, id",
        )?;
        let rows = statement.query_map([], |row| {
            let id = row.get::<_, String>(0)?;
            let kind = match row.get::<_, String>(1)?.as_str() {
                "session" => ReviewKind::Session,
                "entity" => ReviewKind::Entity,
                "ingest" => ReviewKind::Ingest,
                "objection" => ReviewKind::Objection,
                "lint" => ReviewKind::Lint,
                _ => ReviewKind::Lint,
            };
            let payload = parse_json(row.get::<_, String>(2)?);
            Ok(ReviewItem {
                id,
                kind,
                summary: string_at(&payload, "/summary", "Review item"),
                detail: optional_string_at(&payload, "/detail"),
                source_artifact_id: optional_string_at(&payload, "/sourceArtifactId")
                    .or_else(|| optional_string_at(&payload, "/entity/sourceArtifactId")),
                unverified_display_name: optional_string_at(&payload, "/unverifiedDisplayName"),
                objection_anchor: payload
                    .get("objectionAnchor")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
                from_session: payload
                    .get("fromSession")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn open_session(
        &self,
        actor_id: &str,
        input: OpenSessionInput,
    ) -> anyhow::Result<SessionReadModel> {
        validate_non_empty("anchor_block_id", &input.anchor_block_id)?;
        let mut connection = self.open_workspace_connection()?;
        let title = anchor_title(&connection, &input.anchor_block_id)?;
        let thread_id = new_id("thread");
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into operations.thread
                (id, owner_id, title, state, anchor_block_id, quote, stance, created_at, updated_at)
             values
                (?1, ?2, ?3, 'active', ?4, ?5, ?6, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                thread_id,
                actor_id,
                title,
                input.anchor_block_id,
                input.quote,
                input.stance.as_ref().map(Stance::as_str),
            ],
        )?;
        transaction.commit()?;
        self.read_session(actor_id, &thread_id)
    }

    pub fn read_session(
        &self,
        actor_id: &str,
        thread_id: &str,
    ) -> anyhow::Result<SessionReadModel> {
        validate_non_empty("thread_id", thread_id)?;
        let connection = self.open_workspace_connection()?;
        ensure_thread_owned(&connection, thread_id, actor_id)?;
        read_session_model(&connection, thread_id)
    }

    pub fn append_user_message(
        &self,
        actor_id: &str,
        thread_id: &str,
        input: AppendMessageInput,
    ) -> anyhow::Result<SessionReadModel> {
        validate_non_empty("thread_id", thread_id)?;
        validate_non_empty("text", &input.text)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_active_thread_owned(&connection, thread_id, actor_id)?;
        let transaction = connection.transaction()?;
        let message_id = new_id("message");
        transaction.execute(
            "insert into operations.message (id, thread_id, actor_id, role, content, created_at)
             values (?1, ?2, ?3, 'user', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![message_id, thread_id, actor_id, input.text],
        )?;
        transaction.execute(
            "update operations.thread
             set updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where id = ?1",
            params![thread_id],
        )?;
        let queued_reply_id = transaction
            .query_row(
                "select id from operations.worker_job
                 where kind = 'agent_reply' and target_id = ?1 and state = 'queued'
                 order by rowid limit 1",
                params![thread_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let payload = json!({
            "thread_id": thread_id,
            "through_message_id": message_id,
        })
        .to_string();
        if let Some(job_id) = queued_reply_id {
            transaction.execute(
                "update operations.worker_job
                 set payload_json = ?2, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 where id = ?1",
                params![job_id, payload],
            )?;
        } else {
            transaction.execute(
                "insert into operations.worker_job
                    (id, kind, state, payload_json, target_id, created_at, updated_at)
                 values
                    (?1, 'agent_reply', 'queued', ?2, ?3, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                params![new_id("job"), payload, thread_id],
            )?;
        }
        transaction.commit()?;
        self.read_session(actor_id, thread_id)
    }

    pub fn append_agent_message(&self, thread_id: &str, text: &str) -> anyhow::Result<()> {
        validate_non_empty("thread_id", thread_id)?;
        validate_non_empty("text", text)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "insert into operations.message (id, thread_id, actor_id, role, content, created_at)
             values (?1, ?2, ?3, 'agent', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![new_id("message"), thread_id, AGENT_ACTOR_ID, text],
        )?;
        Ok(())
    }

    pub fn discard_session(
        &self,
        actor_id: &str,
        thread_id: &str,
    ) -> anyhow::Result<SessionReadModel> {
        validate_non_empty("thread_id", thread_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.thread
             set state = 'discarded',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where id = ?1 and owner_id = ?2 and state = 'active'",
            params![thread_id, actor_id],
        )?;
        self.read_session(actor_id, thread_id)
    }

    pub fn commit_session(&self, actor_id: &str, thread_id: &str) -> anyhow::Result<ReviewItem> {
        validate_non_empty("thread_id", thread_id)?;
        let mut connection = self.open_workspace_connection()?;
        ensure_active_thread_owned(&connection, thread_id, actor_id)?;
        let session = read_session_model(&connection, thread_id)?;
        if session.state != "active" {
            return Err(StoreError::Validation("session is not active".to_string()).into());
        }
        if session.turns.is_empty() && session.stance.is_none() {
            return Err(StoreError::Validation("session has nothing to commit".to_string()).into());
        }

        let event_id = new_id("event");
        let review_id = new_id("review");
        let mut payload = session_commit_payload(&session);
        payload["spaceId"] =
            Value::String(space_for_anchor(&connection, &session.anchor.block_id)?);
        let review_payload = review_payload_from_session(&session);
        let transaction = connection.transaction()?;
        transaction.execute(
            "insert into events.event
                (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
             values
                (?1, ?2, ?3, 'session_commit', 'proposed', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![event_id, actor_id, session.anchor.block_id, payload.to_string()],
        )?;
        transaction.execute(
            "insert into operations.attention_item
                (id, owner_id, kind, target_id, state, payload_json, created_at)
             values
                (?1, ?2, 'session', ?3, 'pending', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![review_id, actor_id, event_id, review_payload.to_string()],
        )?;
        transaction.execute(
            "update operations.thread
             set state = 'committed',
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where id = ?1",
            params![thread_id],
        )?;
        transaction.commit()?;
        self.read_review()?
            .into_iter()
            .find(|item| item.id == review_id)
            .ok_or_else(|| StoreError::NotFound("review item not found".to_string()).into())
    }

    pub fn reject_review_item(&self, actor_id: &str, review_id: &str) -> anyhow::Result<()> {
        validate_non_empty("review_id", review_id)?;
        let mut connection = self.open_workspace_connection()?;
        let event_id = connection
            .query_row(
                "select target_id from operations.attention_item where id = ?1 and state = 'pending'",
                params![review_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("review item not found".to_string()))?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "update events.event set decision_state = 'rejected',
             payload_json = json_set(payload_json, '$.reviewedBy', ?2) where id = ?1 and decision_state = 'proposed'",
            params![event_id, actor_id],
        )?;
        transaction.execute(
            "update operations.attention_item set state = 'resolved',
             resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') where id = ?1",
            params![review_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn accept_review_item(
        &self,
        actor_id: &str,
        review_id: &str,
    ) -> anyhow::Result<SynthesisDoc> {
        validate_non_empty("review_id", review_id)?;
        let mut connection = self.open_workspace_connection()?;
        let (event_id, attention_kind, review_payload) = connection
            .query_row(
                "select target_id, kind, payload_json
                 from operations.attention_item
                 where id = ?1 and state = 'pending'",
                params![review_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        parse_json(row.get::<_, String>(2)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("review item not found".to_string()))?;
        let (event_kind, event_actor_id, event_payload) = connection
            .query_row(
                "select kind, actor_id, payload_json
                 from events.event
                 where id = ?1 and decision_state = 'proposed'",
                params![event_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        parse_json(row.get::<_, String>(2)?),
                    ))
                },
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("proposed event not found".to_string()))?;

        match (attention_kind.as_str(), event_kind.as_str()) {
            ("session", "session_commit") => {
                let anchor_block_id = string_at(&event_payload, "/anchorBlockId", "");
                validate_non_empty("anchor_block_id", &anchor_block_id)?;
                let stance = optional_string_at(&event_payload, "/stance");
                let commit_summary =
                    string_at(&review_payload, "/summary", "integrated your session");
                let space_id = space_for_anchor(&connection, &anchor_block_id)?;
                let mut doc = read_doc_model(&connection, &space_id)?;
                doc.version += 1;
                doc.compiled_at = now_label();
                insert_commit_node(
                    &mut doc.nodes,
                    &anchor_block_id,
                    doc.version,
                    &commit_summary,
                );

                let transaction = connection.transaction()?;
                if let Some(stance) = stance {
                    if entity_exists(&transaction, &anchor_block_id)? {
                        transaction.execute(
                            "insert into stance (id, actor_id, target_id, axis, value, created_at)
                             values (?1, ?2, ?3, 'endorse', ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                            params![new_id("stance"), actor_id, anchor_block_id, stance],
                        )?;
                    }
                }
                transaction.execute(
                    "update events.event
                     set decision_state = 'decided'
                     where id = ?1",
                    params![event_id],
                )?;
                transaction.execute(
                    "update operations.attention_item
                     set state = 'resolved',
                         resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     where id = ?1",
                    params![review_id],
                )?;
                write_doc_model(&transaction, &doc)?;
                transaction.commit()?;
                Ok(doc)
            }
            ("entity", "conditional_stance_requested") => {
                let space_id = string_at(&event_payload, "/spaceId", "");
                let target_entity_id = string_at(&event_payload, "/targetEntityId", "");
                let condition_entity_id = string_at(&event_payload, "/conditionEntityId", "");
                let evidence_artifact_id = string_at(&event_payload, "/evidenceArtifactId", "");
                let declarer_id = event_actor_id.as_deref().ok_or_else(|| {
                    StoreError::Validation(
                        "conditional stance has no authenticated declarer".to_string(),
                    )
                })?;
                let terms = string_at(&review_payload, "/conditionalStance/terms", "");
                validate_non_empty("space_id", &space_id)?;
                validate_non_empty("target_entity_id", &target_entity_id)?;
                validate_non_empty("condition_entity_id", &condition_entity_id)?;
                validate_non_empty("evidence_artifact_id", &evidence_artifact_id)?;
                validate_non_empty("terms", &terms)?;
                if terms.trim() != terms {
                    return Err(StoreError::Validation(
                        "condition terms must remain exact".to_string(),
                    )
                    .into());
                }
                let owns_stance = connection
                    .query_row(
                        "select 1 from stance where actor_id = ?1 and target_id = ?2 and axis = 'endorse'
                         order by rowid desc limit 1",
                        params![declarer_id, target_entity_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !owns_stance {
                    return Err(StoreError::Validation(
                        "a condition can only be declared on your own stance".to_string(),
                    )
                    .into());
                }
                let duplicate = connection
                    .query_row(
                        "select 1 from contract where kind = 'conditional_stance'
                         and actor_id = ?1 and target_entity_id = ?2 and terms = ?3",
                        params![declarer_id, target_entity_id, terms],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if duplicate {
                    return Err(StoreError::Validation(
                        "this conditional stance is already declared".to_string(),
                    )
                    .into());
                }
                let target_space = connection
                    .query_row(
                        "select space_id from entity where id = ?1 and role = 'issue'",
                        params![target_entity_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if target_space.as_deref() != Some(space_id.as_str()) {
                    return Err(StoreError::Validation(
                        "condition target no longer belongs to this space".to_string(),
                    )
                    .into());
                }
                let evidence = connection
                    .query_row(
                        "select content, metadata_json from artifact where id = ?1 and kind = 'source'",
                        params![evidence_artifact_id],
                        |row| Ok((row.get::<_, String>(0)?, parse_json(row.get::<_, String>(1)?))),
                    )
                    .optional()?;
                let Some((evidence_content, evidence_metadata)) = evidence else {
                    return Err(StoreError::Validation(
                        "conditional stance evidence no longer exists".to_string(),
                    )
                    .into());
                };
                if !evidence_content.contains(&terms)
                    || string_at(&evidence_metadata, "/submittedBy", "") != declarer_id
                    || string_at(&evidence_metadata, "/spaceId", "") != space_id
                {
                    return Err(StoreError::Validation(
                        "conditional stance no longer has exact attributed evidence".to_string(),
                    )
                    .into());
                }

                let transaction = connection.transaction()?;
                transaction.execute(
                    "insert into entity (id, space_id, content, role, state, created_by, confirmed_by)
                     values (?1, ?2, ?3, 'condition', 'open', ?4, ?5)",
                    params![condition_entity_id, space_id, terms, declarer_id, actor_id],
                )?;
                transaction.execute(
                    "insert into contract
                        (id, parties_json, terms, status, kind, actor_id, target_entity_id,
                         condition_entity_id, evidence_artifact_id)
                     values (?1, ?2, ?3, 'open', 'conditional_stance', ?4, ?5, ?6, ?7)",
                    params![
                        new_id("contract"),
                        json!([declarer_id]).to_string(),
                        terms,
                        declarer_id,
                        target_entity_id,
                        condition_entity_id,
                        evidence_artifact_id,
                    ],
                )?;
                transaction.execute(
                    "update events.event set decision_state = 'decided',
                     payload_json = json_set(payload_json, '$.reviewedBy', ?2)
                     where id = ?1 and decision_state = 'proposed'",
                    params![event_id, actor_id],
                )?;
                transaction.execute(
                    "update operations.attention_item set state = 'resolved',
                     resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') where id = ?1",
                    params![review_id],
                )?;
                let doc = recompile_doc_projection(&transaction, &space_id)?;
                transaction.commit()?;
                Ok(doc)
            }
            ("entity", "entity_create_requested") => {
                let space_id = string_at(&event_payload, "/spaceId", "");
                let entity_id = string_at(&event_payload, "/entityId", "");
                let role = string_at(&event_payload, "/role", "");
                let state = string_at(&event_payload, "/state", "");
                let text = string_at(&review_payload, "/entity/text", "");
                validate_non_empty("space_id", &space_id)?;
                validate_non_empty("entity_id", &entity_id)?;
                validate_ontology_role(&role)?;
                validate_non_empty("state", &state)?;
                validate_non_empty("text", &text)?;

                let transaction = connection.transaction()?;
                transaction.execute(
                    "insert into entity (id, space_id, content, role, state, created_by, confirmed_by)
                     values (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                    params![entity_id, space_id, text, role, state, actor_id],
                )?;
                transaction.execute(
                    "update events.event
                     set decision_state = 'decided'
                     where id = ?1",
                    params![event_id],
                )?;
                transaction.execute(
                    "update operations.attention_item
                     set state = 'resolved',
                         resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     where id = ?1",
                    params![review_id],
                )?;
                let doc = recompile_doc_projection(&transaction, &space_id)?;
                transaction.commit()?;
                Ok(doc)
            }
            ("objection", "objection_submitted") => {
                let space_id = string_at(&event_payload, "/spaceId", "");
                let source_artifact_id = string_at(&event_payload, "/sourceArtifactId", "");
                validate_non_empty("space_id", &space_id)?;
                validate_non_empty("source_artifact_id", &source_artifact_id)?;
                let artifact_space = connection
                    .query_row(
                        "select json_extract(metadata_json, '$.spaceId') from artifact
                         where id = ?1 and kind = 'annotation'
                           and json_extract(metadata_json, '$.inputKind') = 'objection'",
                        params![source_artifact_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?;
                if artifact_space.as_deref() != Some(space_id.as_str()) {
                    return Err(StoreError::Validation(
                        "objection artifact no longer matches its review item".to_string(),
                    )
                    .into());
                }
                let transaction = connection.transaction()?;
                transaction.execute(
                    "update events.event set decision_state = 'decided',
                     payload_json = json_set(payload_json, '$.reviewedBy', ?2)
                     where id = ?1 and decision_state = 'proposed'",
                    params![event_id, actor_id],
                )?;
                transaction.execute(
                    "update operations.attention_item set state = 'resolved',
                     resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                     payload_json = json_set(payload_json, '$.decidedAfterEventRowid',
                         (select coalesce(max(rowid), 0) from events.event))
                     where id = ?1",
                    params![review_id],
                )?;
                let doc = read_doc_model(&transaction, &space_id)?;
                transaction.commit()?;
                Ok(doc)
            }
            ("ingest", "lesswrong_landscape_proposed") => {
                let space_id = string_at(&event_payload, "/spaceId", "");
                validate_non_empty("space_id", &space_id)?;
                let findings = review_payload
                    .get("findings")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        StoreError::Validation("thread review is missing findings".to_string())
                    })?;
                let transaction = connection.transaction()?;
                for finding in findings {
                    let action = string_at(finding, "/action", "");
                    let text = string_at(finding, "/text", "");
                    let reasoning = string_at(finding, "/reasoning", "");
                    validate_non_empty("finding text", &text)?;
                    validate_non_empty("finding reasoning", &reasoning)?;
                    let entity_id = match action.as_str() {
                        "new" => {
                            let entity_id = string_at(finding, "/id", "");
                            validate_non_empty("entity_id", &entity_id)?;
                            transaction.execute(
                                "insert into entity (id, space_id, content, role, state, created_by, confirmed_by)
                                 values (?1, ?2, ?3, 'issue', 'open', ?4, ?5)",
                                params![entity_id, space_id, text, AGENT_ACTOR_ID, actor_id],
                            )?;
                            entity_id
                        }
                        "attach" => {
                            let entity_id = string_at(finding, "/targetEntityId", "");
                            let expected_text = string_at(finding, "/targetText", "");
                            let current_text = transaction.query_row(
                                "select content from entity where id = ?1 and space_id = ?2 and role = 'issue'",
                                params![entity_id, space_id], |row| row.get::<_, String>(0),
                            ).optional()?;
                            if current_text.as_deref() != Some(expected_text.as_str()) {
                                return Err(StoreError::Validation(
                                    "attachment target changed while awaiting review; refresh and merge again".to_string(),
                                ).into());
                            }
                            entity_id
                        }
                        _ => {
                            return Err(StoreError::Validation(
                                "unknown landscape delta action".to_string(),
                            )
                            .into());
                        }
                    };
                    let source_ids = finding
                        .get("sourceArtifactIds")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>();
                    for source_id in &source_ids {
                        transaction.execute(
                            "insert into relation (id, from_id, to_id, kind, created_by, confirmed_by)
                             values (?1, ?2, ?3, 'derived_from', ?4, ?5)",
                            params![new_id("relation"), entity_id, source_id, AGENT_ACTOR_ID, actor_id],
                        )?;
                    }
                    for stance in finding
                        .get("stances")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let holder_id = string_at(stance, "/actorId", "");
                        let value = string_at(stance, "/value", "");
                        let evidence_artifact_id = string_at(stance, "/evidenceArtifactId", "");
                        let stated_confidence = optional_string_at(stance, "/statedConfidence");
                        let evidence = transaction
                            .query_row(
                                "select content, metadata_json from artifact where id = ?1",
                                params![evidence_artifact_id],
                                |row| {
                                    Ok((
                                        row.get::<_, String>(0)?,
                                        parse_json(row.get::<_, String>(1)?),
                                    ))
                                },
                            )
                            .optional()?;
                        let Some((evidence_content, evidence_metadata)) = evidence else {
                            return Err(StoreError::Validation(
                                "stance evidence artifact no longer exists".to_string(),
                            )
                            .into());
                        };
                        let authored_at = string_at(&evidence_metadata, "/postedAt", "");
                        if holder_id.is_empty()
                            || !matches!(value.as_str(), "endorse" | "dispute")
                            || !source_ids.contains(&evidence_artifact_id.as_str())
                            || string_at(&evidence_metadata, "/authorId", "") != holder_id
                            || authored_at.is_empty()
                            || stated_confidence.as_ref().is_some_and(|confidence| {
                                confidence.trim().is_empty()
                                    || confidence.trim() != confidence
                                    || !looks_like_stated_confidence(confidence)
                                    || !evidence_content.contains(confidence)
                            })
                        {
                            return Err(StoreError::Validation(
                                "reviewed stance no longer has valid exact authored evidence"
                                    .to_string(),
                            )
                            .into());
                        }
                        transaction.execute(
                            "insert or ignore into stance
                                (id, actor_id, target_id, axis, value, evidence_artifact_id,
                                 stated_confidence, created_at)
                             values (?1, ?2, ?3, 'endorse', ?4, ?5, ?6,
                                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                            params![
                                new_id("stance"),
                                holder_id,
                                entity_id,
                                value,
                                evidence_artifact_id,
                                stated_confidence
                            ],
                        )?;
                        for falsifier in stance
                            .get("falsifiers")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            let quote = string_at(falsifier, "/quote", "");
                            let condition = string_at(falsifier, "/condition", "");
                            if quote.trim().is_empty()
                                || quote.trim() != quote
                                || condition.trim().is_empty()
                                || condition.trim() != condition
                                || !evidence_content.contains(&quote)
                            {
                                return Err(StoreError::Validation(
                                    "reviewed falsifier no longer has exact authored evidence"
                                        .to_string(),
                                )
                                .into());
                            }
                            let duplicate = transaction
                                .query_row(
                                    "select 1 from contract where kind = 'falsifier'
                                     and actor_id = ?1 and target_entity_id = ?2
                                     and evidence_artifact_id = ?3 and terms = ?4",
                                    params![holder_id, entity_id, evidence_artifact_id, quote],
                                    |_| Ok(()),
                                )
                                .optional()?
                                .is_some();
                            if duplicate {
                                continue;
                            }
                            let condition_entity_id = new_id("entity");
                            transaction.execute(
                                "insert into entity
                                    (id, space_id, content, role, state, created_by, confirmed_by)
                                 values (?1, ?2, ?3, 'condition', 'open', ?4, ?5)",
                                params![
                                    condition_entity_id,
                                    space_id,
                                    condition,
                                    AGENT_ACTOR_ID,
                                    actor_id,
                                ],
                            )?;
                            transaction.execute(
                                "insert into contract
                                    (id, parties_json, terms, status, kind, actor_id,
                                     target_entity_id, condition_entity_id, evidence_artifact_id)
                                 values (?1, ?2, ?3, 'open', 'falsifier', ?4, ?5, ?6, ?7)",
                                params![
                                    new_id("contract"),
                                    json!([holder_id]).to_string(),
                                    quote,
                                    holder_id,
                                    entity_id,
                                    condition_entity_id,
                                    evidence_artifact_id,
                                ],
                            )?;
                        }
                    }
                    let target_text = transaction.query_row(
                        "select content from entity where id = ?1",
                        params![entity_id],
                        |row| row.get::<_, String>(0),
                    )?;
                    transaction.execute(
                        "insert into events.event
                            (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
                         values (?1, ?2, ?3, 'lesswrong_landscape_delta_accepted', 'decided', ?4,
                                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
                        params![new_id("event"), actor_id, entity_id, json!({
                            "spaceId": space_id,
                            "action": action,
                            "inputText": text,
                            "reasoning": reasoning,
                            "targetPositionText": target_text,
                            "sourceArtifactId": source_ids.first(),
                            "sourceArtifactIds": source_ids,
                        }).to_string()],
                    )?;
                }
                transaction.execute(
                    "update events.event set decision_state = 'decided' where id = ?1",
                    params![event_id],
                )?;
                transaction.execute(
                    "update operations.attention_item set state = 'resolved',
                     resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') where id = ?1",
                    params![review_id],
                )?;
                let doc = recompile_doc_projection(&transaction, &space_id)?;
                transaction.commit()?;
                Ok(doc)
            }
            ("ingest", "ingest_entity_proposed") => {
                let space_id = string_at(&event_payload, "/spaceId", "");
                let entity_id = string_at(&event_payload, "/entityId", "");
                let role = string_at(&event_payload, "/role", "");
                let state = string_at(&event_payload, "/state", "");
                let source_artifact_id = string_at(&event_payload, "/sourceArtifactId", "");
                let text = string_at(&review_payload, "/entity/text", "");
                validate_non_empty("space_id", &space_id)?;
                validate_non_empty("entity_id", &entity_id)?;
                validate_ontology_role(&role)?;
                validate_non_empty("state", &state)?;
                validate_non_empty("source_artifact_id", &source_artifact_id)?;
                validate_non_empty("text", &text)?;

                let transaction = connection.transaction()?;
                transaction.execute(
                    "insert into entity (id, space_id, content, role, state, created_by, confirmed_by)
                     values (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        entity_id,
                        space_id,
                        text,
                        role,
                        state,
                        AGENT_ACTOR_ID,
                        actor_id
                    ],
                )?;
                transaction.execute(
                    "insert into relation
                        (id, from_id, to_id, kind, created_by, confirmed_by)
                     values (?1, ?2, ?3, 'derived_from', ?4, ?5)",
                    params![
                        new_id("relation"),
                        entity_id,
                        source_artifact_id,
                        AGENT_ACTOR_ID,
                        actor_id
                    ],
                )?;
                transaction.execute(
                    "update events.event
                     set decision_state = 'decided'
                     where id = ?1",
                    params![event_id],
                )?;
                transaction.execute(
                    "update operations.attention_item
                     set state = 'resolved',
                         resolved_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     where id = ?1",
                    params![review_id],
                )?;
                let doc = recompile_doc_projection(&transaction, &space_id)?;
                transaction.commit()?;
                Ok(doc)
            }
            _ => Err(StoreError::Validation(format!(
                "unsupported review item kind {attention_kind} for event {event_kind}"
            ))
            .into()),
        }
    }

    pub fn build_agent_reply_context(
        &self,
        thread_id: &str,
        through_message_id: &str,
        last_n_turns: usize,
    ) -> anyhow::Result<AgentReplyContext> {
        validate_non_empty("thread_id", thread_id)?;
        validate_non_empty("through_message_id", through_message_id)?;
        if last_n_turns == 0 {
            return Err(StoreError::Validation("last_n_turns must be positive".to_string()).into());
        }
        let connection = self.open_workspace_connection()?;
        let session = read_session_model(&connection, thread_id)?;
        let space_id = space_for_anchor(&connection, &session.anchor.block_id)?;
        let doc = read_doc_model(&connection, &space_id)?;
        let anchor_text = doc
            .nodes
            .iter()
            .find(|node| node_id(node) == session.anchor.block_id)
            .map(node_text)
            .unwrap_or_else(|| doc.title.clone());
        let surrounding_doc = doc
            .nodes
            .iter()
            .map(node_text)
            .collect::<Vec<_>>()
            .join("\n");
        let cutoff_rowid = connection
            .query_row(
                "select rowid from operations.message where id = ?1 and thread_id = ?2",
                params![through_message_id, thread_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .ok_or_else(|| {
                StoreError::NotFound("agent reply target message not found".to_string())
            })?;
        let mut statement = connection.prepare(
            "select role, content from operations.message
             where thread_id = ?1
               and (rowid <= ?2 or role = 'agent')
             order by rowid desc limit ?3",
        )?;
        let rows = statement.query_map(
            params![thread_id, cutoff_rowid, last_n_turns as i64],
            |row| {
                Ok(AgentReplyMessage {
                    who: match row.get::<_, String>(0)?.as_str() {
                        "agent" => TurnAuthor::Agent,
                        _ => TurnAuthor::You,
                    },
                    text: row.get(1)?,
                })
            },
        )?;
        let mut prior_messages = rows.collect::<Result<Vec<_>, _>>()?;
        prior_messages.reverse();
        Ok(AgentReplyContext {
            thread_id: thread_id.to_string(),
            anchor_title: session.anchor.title,
            anchor_text,
            quote: session.anchor.quote,
            stance: session.stance,
            surrounding_doc,
            prior_messages,
        })
    }

    pub fn claim_next_worker_job(&self) -> anyhow::Result<Option<WorkerJob>> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        let job = transaction
            .query_row(
                "select id, kind, payload_json, target_id
                 from operations.worker_job
                 where state = 'queued'
                   and (json_extract(payload_json, '$.auto') is not 1
                        or (kind = 'society_newspaper'
                            and julianday(updated_at) <= julianday('now', '-5 seconds'))
                        or (kind != 'society_newspaper'
                            and julianday(updated_at) <= julianday('now', '-1 second')))
                   and (kind != 'lesswrong_thread_ingest' or not exists (
                        select 1 from operations.attention_item pending
                        join events.event proposed on proposed.id = pending.target_id
                        where pending.state = 'pending'
                          and proposed.kind = 'lesswrong_landscape_proposed'
                          and json_extract(proposed.payload_json, '$.spaceId') = json_extract(worker_job.payload_json, '$.spaceId')
                   ))
                 order by created_at, rowid
                 limit 1",
                [],
                |row| {
                    Ok(WorkerJob {
                        id: row.get(0)?,
                        kind: row.get(1)?,
                        payload_json: row.get(2)?,
                        target_id: row.get(3)?,
                    })
                },
            )
            .optional()?;

        let Some(job) = job else {
            transaction.commit()?;
            return Ok(None);
        };

        transaction.execute(
            "update operations.worker_job
             set state = 'running',
                 claimed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = null
             where id = ?1 and state = 'queued'",
            params![job.id],
        )?;
        transaction.commit()?;
        Ok(Some(job))
    }

    pub fn requeue_running_worker_jobs(&self) -> anyhow::Result<usize> {
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction()?;
        transaction.execute(
            "update operations.worker_attempt
             set state = 'failed',
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = 'worker process interrupted before completion'
             where state = 'running'
               and job_id in (select id from operations.worker_job where state = 'running')",
            [],
        )?;
        let changed = transaction.execute(
            "update operations.worker_job
             set state = 'queued',
                 claimed_at = null,
                 finished_at = null,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = null
             where state = 'running'",
            [],
        )?;
        transaction.commit()?;
        Ok(changed)
    }

    pub fn start_worker_attempt(&self, job_id: &str) -> anyhow::Result<WorkerAttempt> {
        validate_non_empty("job_id", job_id)?;
        let connection = self.open_workspace_connection()?;
        let attempt = WorkerAttempt {
            id: new_id("attempt"),
            job_id: job_id.to_string(),
        };
        connection.execute(
            "insert into operations.worker_attempt (id, job_id, state, started_at)
             values (?1, ?2, 'running', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![attempt.id, attempt.job_id],
        )?;
        Ok(attempt)
    }

    pub fn record_worker_attempt_llm_call(
        &self,
        attempt_id: &str,
        call: &Value,
    ) -> anyhow::Result<()> {
        validate_non_empty("attempt_id", attempt_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_attempt
             set llm_calls_json = json_insert(llm_calls_json, '$[#]', json(?2))
             where id = ?1",
            params![attempt_id, call.to_string()],
        )?;
        Ok(())
    }

    pub fn read_cost_summary(&self) -> anyhow::Result<CostSummary> {
        let connection = self.open_workspace_connection()?;
        let overall = connection.query_row(
            "select count(*),
                    coalesce(sum(cast(json_extract(call.value, '$.inputTokens') as integer)), 0),
                    coalesce(sum(cast(json_extract(call.value, '$.outputTokens') as integer)), 0),
                    coalesce(sum(cast(json_extract(call.value, '$.costUsd') as real)), 0.0)
             from operations.worker_attempt attempt, json_each(attempt.llm_calls_json) call",
            [],
            |row| {
                Ok(CostTotals {
                    llm_calls: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cost_usd: row.get(3)?,
                })
            },
        )?;
        let mut statement = connection.prepare(
            "select space.id, count(*),
                    sum(cast(json_extract(call.value, '$.inputTokens') as integer)),
                    sum(cast(json_extract(call.value, '$.outputTokens') as integer)),
                    sum(cast(json_extract(call.value, '$.costUsd') as real))
             from operations.worker_attempt attempt
             join operations.worker_job job on job.id = attempt.job_id
             join space on space.id = coalesce(json_extract(job.payload_json, '$.spaceId'), job.target_id)
             join json_each(attempt.llm_calls_json) call
             group by space.id order by space.id",
        )?;
        let spaces = statement
            .query_map([], |row| {
                Ok(SpaceCostTotals {
                    space_id: row.get(0)?,
                    totals: CostTotals {
                        llm_calls: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cost_usd: row.get(4)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut stage_statement = connection.prepare(
            "select coalesce(json_extract(call.value, '$.stage'), 'other'), count(*),
                    sum(cast(json_extract(call.value, '$.inputTokens') as integer)),
                    sum(cast(json_extract(call.value, '$.outputTokens') as integer)),
                    sum(cast(json_extract(call.value, '$.costUsd') as real))
             from operations.worker_attempt attempt, json_each(attempt.llm_calls_json) call
             group by 1 order by 1",
        )?;
        let stages = stage_statement
            .query_map([], |row| {
                Ok(StageCostTotals {
                    stage: row.get(0)?,
                    totals: CostTotals {
                        llm_calls: row.get(1)?,
                        input_tokens: row.get(2)?,
                        output_tokens: row.get(3)?,
                        cost_usd: row.get(4)?,
                    },
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut model_statement = connection.prepare(
            "select distinct json_extract(call.value, '$.servedModel')
             from operations.worker_attempt attempt, json_each(attempt.llm_calls_json) call
             where json_extract(call.value, '$.servedModel') is not null order by 1",
        )?;
        let served_models = model_statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(CostSummary {
            overall,
            spaces,
            stages,
            served_models,
        })
    }

    pub fn finish_worker_attempt(
        &self,
        attempt_id: &str,
        result_summary: &str,
    ) -> anyhow::Result<()> {
        validate_non_empty("attempt_id", attempt_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_attempt
             set state = 'succeeded',
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 result_summary = ?2
             where id = ?1",
            params![attempt_id, result_summary],
        )?;
        Ok(())
    }

    pub fn fail_worker_attempt(&self, attempt_id: &str, error: &str) -> anyhow::Result<()> {
        validate_non_empty("attempt_id", attempt_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_attempt
             set state = 'failed',
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = ?2
             where id = ?1",
            params![attempt_id, error],
        )?;
        Ok(())
    }

    pub fn mark_worker_job_succeeded(&self, job_id: &str) -> anyhow::Result<()> {
        validate_non_empty("job_id", job_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_job
             set state = 'succeeded',
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = null
             where id = ?1",
            params![job_id],
        )?;
        Ok(())
    }

    pub fn mark_worker_job_failed(&self, job_id: &str, error: &str) -> anyhow::Result<()> {
        validate_non_empty("job_id", job_id)?;
        let connection = self.open_workspace_connection()?;
        connection.execute(
            "update operations.worker_job
             set state = 'failed',
                 finished_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 error = ?2
             where id = ?1",
            params![job_id, error],
        )?;
        Ok(())
    }

    fn rebuild_compiled_stance_projections(&self) -> anyhow::Result<()> {
        let mut connection = self.open_workspace_connection()?;
        let space_ids = {
            let mut statement = connection.prepare("select space_id from synthesis_doc")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?
        };
        let transaction = connection.transaction()?;
        for space_id in space_ids {
            recompile_doc_projection(&transaction, &space_id)?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn open_workspace_connection(&self) -> anyhow::Result<Connection> {
        let connection = Connection::open(&self.core_path).with_context(|| {
            format!(
                "failed to open core sqlite database {}",
                self.core_path.display()
            )
        })?;
        configure_connection(&connection)?;
        connection.execute(
            "attach database ?1 as events",
            params![self.events_path.to_string_lossy().as_ref()],
        )?;
        connection.execute(
            "attach database ?1 as operations",
            params![self.operations_path.to_string_lossy().as_ref()],
        )?;
        Ok(connection)
    }
}

fn append_event(
    connection: &Connection,
    actor_id: Option<&str>,
    target_id: Option<&str>,
    kind: &str,
    payload: Value,
) -> anyhow::Result<String> {
    let event_id = new_id("event");
    connection.execute(
        "insert into events.event
            (id, actor_id, target_id, kind, decision_state, payload_json, created_at)
         values (?1, ?2, ?3, ?4, 'decided', ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![event_id, actor_id, target_id, kind, payload.to_string()],
    )?;
    Ok(event_id)
}

struct LedgerEntryInput<'a> {
    actor_id: Option<&'a str>,
    recipient_id: Option<&'a str>,
    escrow_id: Option<&'a str>,
    amount: i64,
    reference_id: Option<&'a str>,
    resolution_authority: Option<EscrowResolutionAuthority>,
}

fn append_ledger_entry(
    connection: &Connection,
    kind: &str,
    input: LedgerEntryInput<'_>,
) -> anyhow::Result<String> {
    validate_positive_obols(input.amount)?;
    let ledger_id = new_id("ledger");
    connection.execute(
        "insert into operations.obol_ledger
            (id, kind, actor_id, recipient_id, escrow_id, amount, reference_id,
             resolution_authority, created_at)
         values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            ledger_id,
            kind,
            input.actor_id,
            input.recipient_id,
            input.escrow_id,
            input.amount,
            input.reference_id,
            input
                .resolution_authority
                .map(EscrowResolutionAuthority::as_str)
        ],
    )?;
    Ok(ledger_id)
}

fn create_escrow_in_transaction(
    connection: &Connection,
    payer_id: &str,
    recipient_id: &str,
    amount: i64,
    reference_id: Option<&str>,
    resolution_authority: EscrowResolutionAuthority,
) -> anyhow::Result<String> {
    validate_positive_obols(amount)?;
    ensure_spendable(connection, payer_id, amount)?;
    let escrow_id = new_id("escrow");
    let ledger_id = append_ledger_entry(
        connection,
        "escrow_create",
        LedgerEntryInput {
            actor_id: Some(payer_id),
            recipient_id: Some(recipient_id),
            escrow_id: Some(&escrow_id),
            amount,
            reference_id,
            resolution_authority: Some(resolution_authority),
        },
    )?;
    append_event(
        connection,
        Some(payer_id),
        Some(&escrow_id),
        "obol_escrow_created",
        json!({
            "ledgerId": ledger_id,
            "amount": amount,
            "referenceId": reference_id,
            "resolutionAuthority": resolution_authority.as_str()
        }),
    )?;
    Ok(escrow_id)
}

enum EscrowResolutionAttempt<'a> {
    Payer(&'a str),
    RecipientRead(&'a str),
}

fn finish_escrow_in_transaction(
    connection: &Connection,
    attempt: EscrowResolutionAttempt<'_>,
    escrow_id: &str,
    kind: &str,
) -> anyhow::Result<String> {
    let (payer_id, recipient_id, amount, reference_id, authority) =
        open_escrow_row(connection, escrow_id)?;
    let actor_id = match attempt {
        EscrowResolutionAttempt::Payer(actor_id)
            if authority == EscrowResolutionAuthority::Payer && payer_id == actor_id =>
        {
            actor_id
        }
        EscrowResolutionAttempt::RecipientRead(actor_id)
            if authority == EscrowResolutionAuthority::RecipientRead
                && recipient_id == actor_id
                && kind == "escrow_execute" =>
        {
            actor_id
        }
        _ => {
            return Err(StoreError::Validation(
                "escrow resolution authority does not permit this attempt".to_string(),
            )
            .into());
        }
    };
    let ledger_id = append_ledger_entry(
        connection,
        kind,
        LedgerEntryInput {
            actor_id: Some(&payer_id),
            recipient_id: Some(&recipient_id),
            escrow_id: Some(escrow_id),
            amount,
            reference_id: reference_id.as_deref(),
            resolution_authority: None,
        },
    )?;
    append_event(
        connection,
        Some(actor_id),
        Some(escrow_id),
        if kind == "escrow_release" {
            "obol_escrow_released"
        } else {
            "obol_escrow_executed"
        },
        json!({ "ledgerId": ledger_id, "amount": amount }),
    )?;
    Ok(ledger_id)
}

fn settle_escrow_to_recipient(
    connection: &Connection,
    recipient_actor_id: &str,
    escrow_id: &str,
) -> anyhow::Result<String> {
    finish_escrow_in_transaction(
        connection,
        EscrowResolutionAttempt::RecipientRead(recipient_actor_id),
        escrow_id,
        "escrow_execute",
    )
}

fn open_escrow_row(
    connection: &Connection,
    escrow_id: &str,
) -> anyhow::Result<(
    String,
    String,
    i64,
    Option<String>,
    EscrowResolutionAuthority,
)> {
    let (payer_id, recipient_id, amount, reference_id, authority) = connection
        .query_row(
            "select actor_id, recipient_id, amount, reference_id, resolution_authority
             from operations.obol_ledger
             where escrow_id = ?1 and kind = 'escrow_create'",
            params![escrow_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("open escrow not found".to_string()))?;
    let created = (
        payer_id,
        recipient_id,
        amount,
        reference_id,
        parse_escrow_resolution_authority(&authority)?,
    );
    let closed = connection
        .query_row(
            "select 1 from operations.obol_ledger
             where escrow_id = ?1 and kind in ('escrow_release', 'escrow_execute')",
            params![escrow_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if closed {
        return Err(StoreError::Validation("escrow is already closed".to_string()).into());
    }
    Ok(created)
}

fn read_escrow(connection: &Connection, escrow_id: &str) -> anyhow::Result<EscrowReadModel> {
    connection
        .query_row(
            "select payer.address, recipient.address, created.amount, created.reference_id,
                    created.resolution_authority, coalesce((
                        select case terminal.kind
                            when 'escrow_release' then 'released'
                            else 'executed'
                        end
                        from operations.obol_ledger terminal
                        where terminal.escrow_id = created.escrow_id
                          and terminal.kind in ('escrow_release', 'escrow_execute')
                    ), 'open')
             from operations.obol_ledger created
             join actor payer on payer.id = created.actor_id
             join actor recipient on recipient.id = created.recipient_id
             where created.escrow_id = ?1 and created.kind = 'escrow_create'",
            params![escrow_id],
            |row| {
                Ok(EscrowReadModel {
                    id: escrow_id.to_string(),
                    payer_address: row.get(0)?,
                    recipient_address: row.get(1)?,
                    amount: row.get(2)?,
                    reference_id: row.get(3)?,
                    resolution_authority: parse_escrow_resolution_authority(
                        &row.get::<_, String>(4)?,
                    )
                    .map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            error.into(),
                        )
                    })?,
                    status: row.get(5)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("escrow not found".to_string()).into())
}

fn bank_snapshot(connection: &Connection) -> anyhow::Result<BankSnapshot> {
    let mut balances = BTreeMap::<String, i64>::new();
    {
        let mut statement = connection.prepare(
            "select id from actor where address is not null and kind in ('agent', 'treasury')",
        )?;
        for actor_id in statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
        {
            balances.insert(actor_id, 0);
        }
    }
    let mut total_minted = 0_i64;
    let mut escrow_amounts = BTreeMap::<String, i64>::new();
    let mut statement = connection.prepare(
        "select kind, actor_id, recipient_id, escrow_id, amount
         from operations.obol_ledger order by created_at, id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, i64>(4)?,
        ))
    })?;
    for row in rows {
        let (kind, actor_id, recipient_id, escrow_id, amount) = row?;
        match kind.as_str() {
            "mint" => {
                total_minted = checked_obol_add(total_minted, amount)?;
                apply_balance(
                    &mut balances,
                    required_id(recipient_id, "mint recipient")?,
                    amount,
                )?;
            }
            "transfer" => {
                apply_balance(
                    &mut balances,
                    required_id(actor_id, "transfer actor")?,
                    -amount,
                )?;
                apply_balance(
                    &mut balances,
                    required_id(recipient_id, "transfer recipient")?,
                    amount,
                )?;
            }
            "escrow_create" => {
                apply_balance(
                    &mut balances,
                    required_id(actor_id, "escrow actor")?,
                    -amount,
                )?;
                escrow_amounts.insert(required_id(escrow_id, "escrow id")?, amount);
            }
            "escrow_release" => {
                apply_balance(
                    &mut balances,
                    required_id(actor_id, "escrow actor")?,
                    amount,
                )?;
                escrow_amounts.remove(&required_id(escrow_id, "escrow id")?);
            }
            "escrow_execute" => {
                apply_balance(
                    &mut balances,
                    required_id(recipient_id, "escrow recipient")?,
                    amount,
                )?;
                escrow_amounts.remove(&required_id(escrow_id, "escrow id")?);
            }
            _ => return Err(anyhow::anyhow!("unknown obol ledger kind {kind}")),
        }
    }
    if balances.values().any(|balance| *balance < 0) {
        return Err(anyhow::anyhow!("obol ledger contains a negative balance"));
    }
    let total_balances = balances
        .values()
        .try_fold(0_i64, |total, balance| checked_obol_add(total, *balance))?;
    let open_escrows = escrow_amounts
        .values()
        .try_fold(0_i64, |total, amount| checked_obol_add(total, *amount))?;
    let mut accounts = Vec::with_capacity(balances.len());
    for (actor_id, balance) in balances {
        accounts.push(ObolAccountBalance {
            address: actor_address(connection, &actor_id)?,
            balance,
        });
    }
    accounts.sort_by(|left, right| left.address.cmp(&right.address));
    Ok(BankSnapshot {
        total_minted,
        total_balances,
        open_escrows,
        accounts,
    })
}

fn assert_bank_conservation(connection: &Connection) -> anyhow::Result<()> {
    let snapshot = bank_snapshot(connection)?;
    let accounted = checked_obol_add(snapshot.total_balances, snapshot.open_escrows)?;
    if snapshot.total_minted > MAX_SAFE_OBOLS {
        return Err(StoreError::Validation(
            "total obol supply exceeds the exact wire-format limit".to_string(),
        )
        .into());
    }
    if snapshot.total_minted != accounted {
        return Err(anyhow::anyhow!(
            "obol conservation violated: minted {} != balances {} + open escrows {}",
            snapshot.total_minted,
            snapshot.total_balances,
            snapshot.open_escrows
        ));
    }
    Ok(())
}

fn ensure_spendable(connection: &Connection, actor_id: &str, amount: i64) -> anyhow::Result<()> {
    validate_positive_obols(amount)?;
    if spendable_balance(connection, actor_id)? < amount {
        return Err(StoreError::Validation("insufficient obol balance".to_string()).into());
    }
    Ok(())
}

fn spendable_balance(connection: &Connection, actor_id: &str) -> anyhow::Result<i64> {
    let snapshot = bank_snapshot(connection)?;
    let address = actor_address(connection, actor_id)?;
    Ok(snapshot
        .accounts
        .iter()
        .find(|account| account.address == address)
        .map(|account| account.balance)
        .unwrap_or_default())
}

fn apply_balance(
    balances: &mut BTreeMap<String, i64>,
    actor_id: String,
    delta: i64,
) -> anyhow::Result<()> {
    let balance = balances.entry(actor_id).or_default();
    *balance = checked_obol_add(*balance, delta)?;
    Ok(())
}

fn checked_obol_add(left: i64, right: i64) -> anyhow::Result<i64> {
    left.checked_add(right)
        .ok_or_else(|| StoreError::Validation("obol amount overflow".to_string()).into())
}

fn required_id(value: Option<String>, field: &str) -> anyhow::Result<String> {
    value.ok_or_else(|| anyhow::anyhow!("obol ledger row is missing {field}"))
}

fn read_ledger_entry(connection: &Connection, ledger_id: &str) -> anyhow::Result<ObolLedgerEntry> {
    connection
        .query_row(
            "select ledger.id, ledger.kind, payer.address, recipient.address,
                    ledger.escrow_id, ledger.amount, ledger.reference_id, ledger.created_at
             from operations.obol_ledger ledger
             left join actor payer on payer.id = ledger.actor_id
             left join actor recipient on recipient.id = ledger.recipient_id
             where ledger.id = ?1",
            params![ledger_id],
            |row| {
                Ok(ObolLedgerEntry {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    actor_address: row.get(2)?,
                    recipient_address: row.get(3)?,
                    escrow_id: row.get(4)?,
                    amount: row.get(5)?,
                    reference_id: row.get(6)?,
                    created_at: row.get(7)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("obol ledger entry not found".to_string()).into())
}

fn actor_id_for_address(connection: &Connection, address: &str) -> anyhow::Result<String> {
    connection
        .query_row(
            "select id from actor where address = ?1 and kind in ('agent', 'treasury')",
            params![address.trim().to_ascii_lowercase()],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("society actor not found".to_string()).into())
}

fn actor_address(connection: &Connection, actor_id: &str) -> anyhow::Result<String> {
    connection
        .query_row(
            "select address from actor where id = ?1 and address is not null and kind in ('agent', 'treasury')",
            params![actor_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("society actor not found".to_string()).into())
}

fn current_week_window(connection: &Connection) -> anyhow::Result<(String, String)> {
    connection
        .query_row(
            "select
                strftime('%Y-%m-%dT00:00:00.000Z', 'now', printf('-%d days', (cast(strftime('%w', 'now') as integer) + 6) % 7)),
                strftime('%Y-%m-%dT00:00:00.000Z', 'now', printf('-%d days', (cast(strftime('%w', 'now') as integer) + 6) % 7), '+7 days')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(Into::into)
}

fn grant_allowance_to_actor(
    connection: &Connection,
    actor_id: &str,
    amount: i64,
    window_start: &str,
    window_end: &str,
) -> anyhow::Result<ObolLedgerEntry> {
    let reference = format!("allowance:{window_start}");
    let ledger_id = append_ledger_entry(
        connection,
        "mint",
        LedgerEntryInput {
            actor_id: None,
            recipient_id: Some(actor_id),
            escrow_id: None,
            amount,
            reference_id: Some(&reference),
            resolution_authority: None,
        },
    )?;
    connection.execute(
        "insert into operations.society_allowance_grant
            (actor_id, window_start, window_end, amount, ledger_id, created_at)
         values (?1, ?2, ?3, ?4, ?5, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![actor_id, window_start, window_end, amount, ledger_id],
    )?;
    append_event(
        connection,
        None,
        Some(actor_id),
        "society_allowance_granted",
        json!({ "ledgerId": ledger_id, "amount": amount, "windowStart": window_start, "windowEnd": window_end }),
    )?;
    read_ledger_entry(connection, &ledger_id)
}

fn read_society_lifecycle(
    connection: &Connection,
    actor_id: &str,
) -> anyhow::Result<Option<SocietyAgentLifecycle>> {
    connection
        .query_row(
            "select phase, wake_number, next_wake_at, updated_at
             from operations.society_agent_lifecycle where actor_id = ?1",
            params![actor_id],
            |row| {
                let phase = match row.get::<_, String>(0)?.as_str() {
                    "awake" => SocietyAgentPhase::Awake,
                    "sleeping" => SocietyAgentPhase::Sleeping,
                    value => {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                format!("unknown society lifecycle phase {value}"),
                            )
                            .into(),
                        ));
                    }
                };
                Ok(SocietyAgentLifecycle {
                    phase,
                    wake_number: row.get(1)?,
                    next_wake_at: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn record_society_lifecycle_in_transaction(
    transaction: &Connection,
    actor_id: &str,
    phase: SocietyAgentPhase,
    wake_number: i64,
    next_wake_at: Option<String>,
) -> anyhow::Result<SocietyAgentLifecycle> {
    if wake_number <= 0 {
        return Err(StoreError::Validation("wakeNumber must be positive".to_string()).into());
    }
    let existing = read_society_lifecycle(transaction, actor_id)?;
    if let Some(existing) = existing {
        if existing.wake_number > wake_number {
            return Err(StoreError::Validation(
                "wake lifecycle cannot move to an earlier wake".to_string(),
            )
            .into());
        }
        if existing.wake_number == wake_number
            && existing.phase == phase
            && existing.next_wake_at == next_wake_at
        {
            return Ok(existing);
        }
        if existing.wake_number == wake_number
            && existing.phase == SocietyAgentPhase::Sleeping
            && phase == SocietyAgentPhase::Sleeping
        {
            return Ok(existing);
        }
        if existing.wake_number == wake_number
            && existing.phase == SocietyAgentPhase::Sleeping
            && phase == SocietyAgentPhase::Awake
        {
            return Err(StoreError::Validation(
                "a settled wake cannot return to awake".to_string(),
            )
            .into());
        }
    }
    transaction.execute(
        "insert into operations.society_agent_lifecycle
            (actor_id, phase, wake_number, next_wake_at, updated_at)
         values (?1, ?2, ?3, ?4, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
         on conflict(actor_id) do update set
            phase = excluded.phase,
            wake_number = excluded.wake_number,
            next_wake_at = excluded.next_wake_at,
            updated_at = excluded.updated_at",
        params![
            actor_id,
            phase.as_str(),
            wake_number,
            next_wake_at.as_deref()
        ],
    )?;
    append_event(
        transaction,
        Some(actor_id),
        Some(actor_id),
        match phase {
            SocietyAgentPhase::Awake => "society_wake_started",
            SocietyAgentPhase::Sleeping => "society_sleep_started",
        },
        json!({ "wakeNumber": wake_number, "nextWakeAt": next_wake_at }),
    )?;
    read_society_lifecycle(transaction, actor_id)?
        .ok_or_else(|| anyhow::anyhow!("recorded lifecycle is missing"))
}

fn request_society_wake_in_transaction(
    transaction: &Connection,
    actor_id: &str,
    injection_id: Option<&str>,
) -> anyhow::Result<()> {
    let Some(existing) = read_society_lifecycle(transaction, actor_id)? else {
        return Ok(());
    };
    if existing.phase != SocietyAgentPhase::Sleeping {
        return Ok(());
    }
    let now = transaction.query_row("select strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |row| {
        row.get::<_, String>(0)
    })?;
    let already_due = existing
        .next_wake_at
        .as_deref()
        .is_some_and(|next_wake_at| {
            transaction
                .query_row(
                    "select julianday(?1) <= julianday(?2)",
                    params![next_wake_at, now],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap_or(false)
        });
    if already_due {
        return Ok(());
    }
    transaction.execute(
        "update operations.society_agent_lifecycle
         set next_wake_at = ?2, updated_at = ?2 where actor_id = ?1",
        params![actor_id, now],
    )?;
    append_event(
        transaction,
        None,
        Some(actor_id),
        "society_manual_wake_requested",
        json!({
            "wakeNumber": existing.wake_number,
            "previousNextWakeAt": existing.next_wake_at,
            "nextWakeAt": now,
            "injectionId": injection_id,
        }),
    )?;
    Ok(())
}

fn validate_injection_message(message: &str) -> anyhow::Result<()> {
    if message.trim().is_empty() {
        return Err(
            StoreError::Validation("injection message must not be empty".to_string()).into(),
        );
    }
    if message.len() > 16_000 {
        return Err(StoreError::Validation(
            "injection message must not exceed 16000 bytes".to_string(),
        )
        .into());
    }
    Ok(())
}

fn validate_signal_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() || name != name.trim() {
        return Err(StoreError::Validation(
            "signal name must be non-empty and have no surrounding whitespace".to_string(),
        )
        .into());
    }
    if name.len() > 200 {
        return Err(
            StoreError::Validation("signal name must not exceed 200 bytes".to_string()).into(),
        );
    }
    Ok(())
}

fn validate_injection_trigger(
    trigger: &ExperimentInjectionTrigger,
) -> anyhow::Result<(&'static str, String)> {
    match trigger {
        ExperimentInjectionTrigger::At { timestamp } => {
            if timestamp.as_bytes().get(10) != Some(&b'T') {
                return Err(StoreError::Validation(
                    "timestamp must be an exact RFC 3339 timestamp".to_string(),
                )
                .into());
            }
            let parsed = OffsetDateTime::parse(timestamp, &Rfc3339).map_err(|_| {
                StoreError::Validation("timestamp must be an exact RFC 3339 timestamp".to_string())
            })?;
            let canonical = parsed.to_offset(UtcOffset::UTC).format(&Rfc3339)?;
            Ok(("at", canonical))
        }
        ExperimentInjectionTrigger::Signal { name } => {
            validate_signal_name(name)?;
            Ok(("signal", name.clone()))
        }
    }
}

fn agent_actor_id_for_address(connection: &Connection, address: &str) -> anyhow::Result<String> {
    connection
        .query_row(
            "select id from actor where address = ?1 and kind = 'agent'",
            params![address],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("society agent not found".to_string()).into())
}

fn resolve_injection_recipients(
    connection: &Connection,
    recipients: &ExperimentInjectionRecipients,
) -> anyhow::Result<Vec<(String, String)>> {
    match recipients {
        ExperimentInjectionRecipients::Addresses { addresses } => {
            let mut unique = BTreeSet::new();
            let mut resolved = Vec::new();
            for address in addresses {
                if address.is_empty() || address != address.trim() {
                    return Err(StoreError::Validation(
                        "recipient addresses must be exact and non-empty".to_string(),
                    )
                    .into());
                }
                if unique.insert(address.clone()) {
                    resolved.push((
                        agent_actor_id_for_address(connection, address)?,
                        address.clone(),
                    ));
                }
            }
            Ok(resolved)
        }
        ExperimentInjectionRecipients::All => {
            let mut statement = connection.prepare(
                "select id, address from actor
                 where kind = 'agent' and address is not null order by address",
            )?;
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(Into::into)
        }
    }
}

fn read_pending_experiment_injections(
    connection: &Connection,
    actor_id: &str,
) -> anyhow::Result<Vec<ExperimentInjectedMessage>> {
    let mut statement = connection.prepare(
        "select injection.id, injection.message
         from operations.experiment_injection_recipient recipient
         join operations.experiment_injection injection on injection.id = recipient.injection_id
         where recipient.actor_id = ?1 and recipient.delivered_at is null
           and injection.state = 'fired'
         order by injection.fired_at, injection.id",
    )?;
    statement
        .query_map(params![actor_id], |row| {
            Ok(ExperimentInjectedMessage {
                injection_id: row.get(0)?,
                message: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn assign_pending_experiment_injections(
    transaction: &Connection,
    actor_id: &str,
    wake_number: i64,
) -> anyhow::Result<()> {
    let messages = read_pending_experiment_injections(transaction, actor_id)?;
    for message in &messages {
        transaction.execute(
            "update operations.experiment_injection_recipient
             set delivered_wake_number = ?3,
                 delivered_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where injection_id = ?1 and actor_id = ?2 and delivered_at is null",
            params![message.injection_id, actor_id, wake_number],
        )?;
        append_event(
            transaction,
            None,
            Some(actor_id),
            "experiment_injection_delivered",
            json!({ "injectionId": message.injection_id, "wakeNumber": wake_number }),
        )?;
    }
    Ok(())
}

fn read_assigned_experiment_injections(
    connection: &Connection,
    actor_id: &str,
    wake_number: i64,
) -> anyhow::Result<Vec<ExperimentInjectedMessage>> {
    let mut statement = connection.prepare(
        "select injection.id, injection.message
         from operations.experiment_injection_recipient recipient
         join operations.experiment_injection injection on injection.id = recipient.injection_id
         where recipient.actor_id = ?1 and recipient.delivered_wake_number = ?2
         order by recipient.delivered_at, injection.id",
    )?;
    statement
        .query_map(params![actor_id, wake_number], |row| {
            Ok(ExperimentInjectedMessage {
                injection_id: row.get(0)?,
                message: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(Into::into)
}

fn fire_due_experiment_injections_in_transaction(
    transaction: &Connection,
) -> anyhow::Result<Vec<ExperimentInjection>> {
    let ids = transaction
        .prepare(
            "select id from operations.experiment_injection
             where state = 'armed' and trigger_kind = 'at'
               and julianday(trigger_value) <= julianday('now')
             order by trigger_value, id",
        )?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    fire_experiment_injections_in_transaction(transaction, ids)
}

fn fire_experiment_injections_in_transaction(
    transaction: &Connection,
    ids: Vec<String>,
) -> anyhow::Result<Vec<ExperimentInjection>> {
    let mut fired = Vec::with_capacity(ids.len());
    for id in ids {
        fire_experiment_injection(transaction, &id)?;
        fired.push(read_experiment_injection(transaction, &id)?);
    }
    Ok(fired)
}

fn fire_experiment_injection(transaction: &Connection, injection_id: &str) -> anyhow::Result<()> {
    let changed = transaction.execute(
        "update operations.experiment_injection
         set state = 'fired', fired_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
         where id = ?1 and state = 'armed'",
        params![injection_id],
    )?;
    if changed == 0 {
        return Ok(());
    }
    append_event(
        transaction,
        None,
        None,
        "experiment_injection_fired",
        json!({ "injectionId": injection_id }),
    )?;
    let make_due = transaction.query_row(
        "select make_due from operations.experiment_injection where id = ?1",
        params![injection_id],
        |row| row.get::<_, bool>(0),
    )?;
    if make_due {
        let mut statement = transaction.prepare(
            "select actor_id from operations.experiment_injection_recipient
             where injection_id = ?1 order by actor_id",
        )?;
        let actor_ids = statement
            .query_map(params![injection_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        for actor_id in actor_ids {
            request_society_wake_in_transaction(transaction, &actor_id, Some(injection_id))?;
        }
    }
    Ok(())
}

fn read_experiment_injection(
    connection: &Connection,
    injection_id: &str,
) -> anyhow::Result<ExperimentInjection> {
    let row = connection
        .query_row(
            "select trigger_kind, trigger_value, message, recipient_kind, make_due,
                    state, armed_at, fired_at
             from operations.experiment_injection where id = ?1",
            params![injection_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("experiment injection not found".to_string()))?;
    let mut statement = connection.prepare(
        "select actor.address, recipient.delivered_wake_number, recipient.delivered_at
         from operations.experiment_injection_recipient recipient
         join actor on actor.id = recipient.actor_id
         where recipient.injection_id = ?1 order by actor.address",
    )?;
    let deliveries = statement
        .query_map(params![injection_id], |row| {
            Ok(ExperimentInjectionRecipient {
                address: row.get(0)?,
                delivered_wake_number: row.get(1)?,
                delivered_at: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let trigger = match row.0.as_str() {
        "at" => ExperimentInjectionTrigger::At { timestamp: row.1 },
        "signal" => ExperimentInjectionTrigger::Signal { name: row.1 },
        other => {
            return Err(anyhow::anyhow!(
                "unknown experiment injection trigger {other}"
            ));
        }
    };
    let recipients = match row.3.as_str() {
        "all" => ExperimentInjectionRecipients::All,
        "addresses" => ExperimentInjectionRecipients::Addresses {
            addresses: deliveries
                .iter()
                .map(|delivery| delivery.address.clone())
                .collect(),
        },
        other => {
            return Err(anyhow::anyhow!(
                "unknown experiment injection recipients {other}"
            ));
        }
    };
    let state = match row.5.as_str() {
        "armed" => ExperimentInjectionState::Armed,
        "fired" => ExperimentInjectionState::Fired,
        other => {
            return Err(anyhow::anyhow!(
                "unknown experiment injection state {other}"
            ));
        }
    };
    Ok(ExperimentInjection {
        id: injection_id.to_string(),
        trigger,
        recipients,
        message: row.2,
        make_due: row.4,
        state,
        armed_at: row.6,
        fired_at: row.7,
        deliveries,
    })
}

fn expire_prior_allowances(
    connection: &Connection,
    actor_id: &str,
    actor_address: &str,
    treasury_id: &str,
    current_window_start: &str,
    ledger_entries: &mut Vec<ObolLedgerEntry>,
) -> anyhow::Result<()> {
    let grants = {
        let mut statement = connection.prepare(
            "select grant.window_start, grant.window_end, grant.amount
             from operations.society_allowance_grant grant
             left join operations.society_allowance_expiry expiry
               on expiry.actor_id = grant.actor_id
              and expiry.window_start = grant.window_start
             where grant.actor_id = ?1
               and grant.window_end <= ?2
               and expiry.actor_id is null
             order by grant.window_start",
        )?;
        statement
            .query_map(params![actor_id, current_window_start], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?
    };

    for (window_start, window_end, granted_amount) in grants {
        let net_outflow = connection.query_row(
            "select coalesce(sum(
                 case
                   when kind in ('transfer', 'escrow_create') and actor_id = ?1 then amount
                   when kind = 'escrow_release' and actor_id = ?1 then -amount
                   else 0
                 end
             ), 0)
             from operations.obol_ledger
             where created_at >= ?2 and created_at < ?3",
            params![actor_id, window_start, window_end],
            |row| row.get::<_, i64>(0),
        )?;
        let spent_amount = net_outflow.max(0).min(granted_amount);
        let unused_amount = granted_amount - spent_amount;
        let balance = bank_snapshot(connection)?
            .accounts
            .into_iter()
            .find(|account| account.address == actor_address)
            .map(|account| account.balance)
            .unwrap_or_default();
        let expired_amount = unused_amount.min(balance);
        let ledger_id = if expired_amount > 0 {
            let reference = format!("allowance-expiry:{window_start}");
            let ledger_id = append_ledger_entry(
                connection,
                "transfer",
                LedgerEntryInput {
                    actor_id: Some(actor_id),
                    recipient_id: Some(treasury_id),
                    escrow_id: None,
                    amount: expired_amount,
                    reference_id: Some(&reference),
                    resolution_authority: None,
                },
            )?;
            ledger_entries.push(read_ledger_entry(connection, &ledger_id)?);
            Some(ledger_id)
        } else {
            None
        };
        connection.execute(
            "insert into operations.society_allowance_expiry
                (actor_id, window_start, window_end, granted_amount, spent_amount,
                 expired_amount, treasury_actor_id, ledger_id, created_at)
             values (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![
                actor_id,
                window_start,
                window_end,
                granted_amount,
                spent_amount,
                expired_amount,
                treasury_id,
                ledger_id.as_deref(),
            ],
        )?;
        append_event(
            connection,
            None,
            Some(actor_id),
            "society_allowance_expired",
            json!({
                "windowStart": window_start,
                "windowEnd": window_end,
                "grantedAmount": granted_amount,
                "spentAmount": spent_amount,
                "expiredAmount": expired_amount,
                "ledgerId": ledger_id,
            }),
        )?;
    }
    Ok(())
}

fn read_society_usage_for_wake(
    connection: &Connection,
    actor_id: &str,
    address: &str,
    wake_number: i64,
) -> anyhow::Result<Option<SocietyUsageRecord>> {
    connection
        .query_row(
            "select id, model, input_tokens, cached_input_tokens, cache_write_input_tokens,
                    output_tokens, reasoning_output_tokens, cost_usd, cost_usd_exact, created_at
             from operations.society_usage where actor_id = ?1 and wake_number = ?2",
            params![actor_id, wake_number],
            |row| {
                Ok(SocietyUsageRecord {
                    id: row.get(0)?,
                    agent_address: address.to_string(),
                    wake_number: Some(wake_number),
                    model: row.get(1)?,
                    input_tokens: row.get(2)?,
                    cached_input_tokens: row.get(3)?,
                    cache_write_input_tokens: row.get(4)?,
                    output_tokens: row.get(5)?,
                    reasoning_output_tokens: row.get(6)?,
                    cost_usd: row.get(7)?,
                    cost_usd_exact: row.get(8)?,
                    created_at: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn read_society_transcript_for_wake(
    connection: &Connection,
    actor_id: &str,
    address: &str,
    wake_number: i64,
) -> anyhow::Result<Option<SocietyTranscript>> {
    connection
        .query_row(
            "select id, session_jsonl, byte_length, sha256, created_at
             from operations.society_transcript where actor_id = ?1 and wake_number = ?2",
            params![actor_id, wake_number],
            |row| {
                let bytes = row.get::<_, Vec<u8>>(1)?;
                Ok(SocietyTranscript {
                    id: row.get(0)?,
                    agent_address: address.to_string(),
                    wake_number,
                    session_jsonl_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    byte_length: row.get(2)?,
                    sha256: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn read_society_admin_agent_summary(
    connection: &Connection,
    bank: &BankSnapshot,
    treasury_id: &str,
    window_start: &str,
    window_end: &str,
    actor: (String, String, String, Option<String>),
) -> anyhow::Result<SocietyAdminAgentSummary> {
    let (actor_id, address, name, description) = actor;
    let balance = bank
        .accounts
        .iter()
        .find(|account| account.address == address)
        .map(|account| account.balance)
        .unwrap_or_default();
    let spent_this_window = connection.query_row(
        "select coalesce(sum(amount), 0) from operations.obol_ledger
         where kind = 'transfer' and actor_id = ?1 and recipient_id = ?2
           and reference_id like 'wake:%' and created_at >= ?3 and created_at < ?4",
        params![actor_id, treasury_id, window_start, window_end],
        |row| row.get(0),
    )?;
    let last_wake = connection
        .query_row(
            "select usage.wake_number, usage.model,
                    max(usage.created_at, transcript.created_at)
             from operations.society_usage usage
             join operations.society_transcript transcript
               on transcript.actor_id = usage.actor_id
              and transcript.wake_number = usage.wake_number
             where usage.actor_id = ?1 and usage.wake_number is not null
             order by usage.wake_number desc limit 1",
            params![actor_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let last_activity_at = connection.query_row(
        "select max(activity_at) from (
             select created_at as activity_at from operations.society_usage where actor_id = ?1
             union all
             select created_at from operations.society_transcript where actor_id = ?1
             union all
             select created_at from operations.society_post where author_actor_id = ?1
             union all
             select read_at from operations.society_post_read where actor_id = ?1
             union all
             select created_at from operations.obol_ledger where actor_id = ?1
         )",
        params![actor_id],
        |row| row.get::<_, Option<String>>(0),
    )?;
    let lifecycle = read_society_lifecycle(connection, &actor_id)?;
    Ok(SocietyAdminAgentSummary {
        address,
        name,
        description,
        balance,
        spent_this_window,
        last_wake_number: last_wake.as_ref().map(|wake| wake.0),
        last_wake_model: last_wake.as_ref().map(|wake| wake.1.clone()),
        last_wake_at: last_wake.map(|wake| wake.2),
        last_activity_at,
        lifecycle,
    })
}

fn society_admin_ledger_entry(
    connection: &Connection,
    entry: ObolLedgerEntry,
) -> anyhow::Result<SocietyAdminLedgerEntry> {
    let (action, from_address, to_address) = match entry.kind.as_str() {
        "mint" => ("minted", entry.actor_address, entry.recipient_address),
        "transfer" => ("paid", entry.actor_address, entry.recipient_address),
        "escrow_create" => ("held for", entry.actor_address, entry.recipient_address),
        "escrow_execute" => (
            "released to",
            Some("Escrow".to_string()),
            entry.recipient_address,
        ),
        "escrow_release" => ("returned", Some("Escrow".to_string()), entry.actor_address),
        other => (other, entry.actor_address, entry.recipient_address),
    };
    let reason = match entry.reference_id.as_deref() {
        Some("birth") => "Starting balance".to_string(),
        Some(reference) if reference.starts_with("allowance-expiry:") => {
            "Unused allowance expired".to_string()
        }
        Some(reference) if reference.starts_with("allowance:") => "Weekly allowance".to_string(),
        Some(reference) if reference.starts_with("wake:") => {
            let wake = reference.rsplit(':').next().unwrap_or(reference);
            format!("Wake {wake}")
        }
        Some(reference) => connection
            .query_row(
                "select headline from operations.society_post where id = ?1",
                params![reference],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| {
                if ["post_", "escrow_", "ledger_", "actor_", "artifact_"]
                    .iter()
                    .any(|prefix| reference.starts_with(prefix))
                {
                    "Recorded reference".to_string()
                } else {
                    reference.to_string()
                }
            }),
        None if entry.kind == "mint" => "Mint".to_string(),
        None if entry.kind.starts_with("escrow_") => "Escrow".to_string(),
        None => "Transfer".to_string(),
    };
    Ok(SocietyAdminLedgerEntry {
        action: action.to_string(),
        from_address,
        to_address,
        amount: entry.amount,
        reason,
        created_at: entry.created_at,
    })
}

fn parse_society_transcript(
    bytes: &[u8],
) -> anyhow::Result<(Vec<SocietyAdminTranscriptEntry>, String)> {
    let mut entries = Vec::new();
    let mut ending = "transcript stored".to_string();
    for line in bytes.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let event: Value = serde_json::from_slice(line)?;
        if event.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = event.get("message").and_then(Value::as_object) else {
            continue;
        };
        let timestamp = event
            .get("timestamp")
            .and_then(Value::as_str)
            .map(str::to_string);
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if role == "assistant" {
            if let Some(reason) = message.get("stopReason").and_then(Value::as_str) {
                ending = match reason {
                    "stop" => "completed",
                    "toolUse" => "ended after tool call",
                    "length" => "output limit",
                    "aborted" => "aborted",
                    "error" => "error",
                    other => other,
                }
                .to_string();
            }
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        match role {
            "user" | "assistant" => {
                for item in content {
                    match item.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = item.get("text").and_then(Value::as_str) {
                                entries.push(SocietyAdminTranscriptEntry {
                                    role: if role == "user" {
                                        SocietyAdminTranscriptRole::User
                                    } else {
                                        SocietyAdminTranscriptRole::Assistant
                                    },
                                    label: None,
                                    text: text.to_string(),
                                    timestamp: timestamp.clone(),
                                    is_error: false,
                                });
                            }
                        }
                        Some("toolCall") => {
                            let arguments = item.get("arguments").cloned().unwrap_or(Value::Null);
                            let text = arguments
                                .as_str()
                                .map(str::to_string)
                                .unwrap_or_else(|| arguments.to_string());
                            entries.push(SocietyAdminTranscriptEntry {
                                role: SocietyAdminTranscriptRole::ToolCall,
                                label: item.get("name").and_then(Value::as_str).map(str::to_string),
                                text,
                                timestamp: timestamp.clone(),
                                is_error: false,
                            });
                        }
                        _ => {}
                    }
                }
            }
            "toolResult" | "tool" => {
                let text = content
                    .iter()
                    .filter_map(|item| item.get("text").and_then(Value::as_str))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !text.is_empty() {
                    entries.push(SocietyAdminTranscriptEntry {
                        role: SocietyAdminTranscriptRole::ToolResult,
                        label: message
                            .get("toolName")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        text,
                        timestamp,
                        is_error: message
                            .get("isError")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
            }
            _ => {}
        }
    }
    Ok((entries, ending))
}

fn parse_board_post_kind(kind: &str) -> anyhow::Result<BoardPostKind> {
    match kind {
        "note" => Ok(BoardPostKind::Note),
        "finding" => Ok(BoardPostKind::Finding),
        "question" => Ok(BoardPostKind::Question),
        "contract" => Ok(BoardPostKind::Contract),
        "verdict" => Ok(BoardPostKind::Verdict),
        "commitment" => Ok(BoardPostKind::Commitment),
        _ => Err(anyhow::anyhow!("unknown board post kind {kind}")),
    }
}

fn parse_escrow_resolution_authority(authority: &str) -> anyhow::Result<EscrowResolutionAuthority> {
    match authority {
        "payer" => Ok(EscrowResolutionAuthority::Payer),
        "recipient_read" => Ok(EscrowResolutionAuthority::RecipientRead),
        _ => Err(anyhow::anyhow!(
            "unknown escrow resolution authority {authority}"
        )),
    }
}

fn board_post_artifact(body: &BoardPostBody) -> (&str, String, Value) {
    match body {
        BoardPostBody::Text { text } => ("text/plain", text.clone(), json!({ "type": "text" })),
        BoardPostBody::Link { url } => (
            "text/uri-list",
            url.trim().to_string(),
            json!({ "type": "link" }),
        ),
        BoardPostBody::File {
            filename,
            media_type,
            content_base64,
        } => (
            media_type.trim(),
            content_base64.clone(),
            json!({ "type": "file", "filename": filename.trim() }),
        ),
    }
}

fn parse_board_post_body(
    media_type: &str,
    content: String,
    metadata_json: &str,
) -> anyhow::Result<BoardPostBody> {
    let metadata: Value = serde_json::from_str(metadata_json)?;
    match metadata.pointer("/body/type").and_then(Value::as_str) {
        Some("text") => Ok(BoardPostBody::Text { text: content }),
        Some("link") => Ok(BoardPostBody::Link { url: content }),
        Some("file") => Ok(BoardPostBody::File {
            filename: metadata
                .pointer("/body/filename")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("board file artifact is missing its filename"))?
                .to_string(),
            media_type: media_type.to_string(),
            content_base64: content,
        }),
        other => Err(anyhow::anyhow!("unknown board post body type {other:?}")),
    }
}

fn read_board_post(
    connection: &Connection,
    post_id: &str,
    reader_actor_id: Option<&str>,
) -> anyhow::Result<BoardPost> {
    let (
        id,
        sequence,
        space_id,
        author_actor_id,
        author_address,
        kind,
        headline,
        media_type,
        content,
        metadata_json,
        parent_post_id,
        parent_sequence,
        created_at,
    ) = connection
        .query_row(
            "select post.id, post.sequence, post.space_id, post.author_actor_id,
                    author.address, post.kind, post.headline, artifact.media_type,
                    artifact.content, artifact.metadata_json, post.parent_post_id,
                    parent.sequence, post.created_at
             from operations.society_post post
             join actor author on author.id = post.author_actor_id
             join artifact on artifact.id = post.artifact_id
             left join operations.society_post parent on parent.id = post.parent_post_id
             where post.id = ?1",
            params![post_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                    row.get::<_, String>(8)?,
                    row.get::<_, String>(9)?,
                    row.get::<_, Option<String>>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                    row.get::<_, String>(12)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("board post not found".to_string()))?;
    let mut recipient_statement = connection.prepare(
        "select recipient.actor_id, actor.address
         from operations.society_post_recipient recipient
         join actor on actor.id = recipient.actor_id
         where recipient.post_id = ?1 order by actor.address",
    )?;
    let recipients = recipient_statement
        .query_map(params![post_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let recipient_addresses = recipients
        .iter()
        .map(|(_, address)| address.clone())
        .collect::<Vec<_>>();
    let attached_obols = connection.query_row(
        "select coalesce(max(amount), 0) from operations.society_attention where post_id = ?1",
        params![post_id],
        |row| row.get::<_, i64>(0),
    )?;
    let sealed = if let Some(reader_actor_id) = reader_actor_id {
        attached_obols > 0
            && author_actor_id != reader_actor_id
            && recipients
                .iter()
                .any(|(actor_id, _)| actor_id == reader_actor_id)
            && connection
                .query_row(
                    "select 1 from operations.society_post_read
                     where post_id = ?1 and actor_id = ?2",
                    params![post_id, reader_actor_id],
                    |_| Ok(()),
                )
                .optional()?
                .is_none()
    } else {
        attached_obols > 0
    };
    let body = if sealed {
        None
    } else {
        Some(parse_board_post_body(&media_type, content, &metadata_json)?)
    };
    Ok(BoardPost {
        id,
        sequence,
        reference: format!("P{sequence}"),
        space_id,
        author_address,
        kind: parse_board_post_kind(&kind)?,
        headline,
        sealed,
        body,
        parent_post_id,
        parent_reference: parent_sequence.map(|sequence| format!("P{sequence}")),
        recipient_addresses,
        attached_obols,
        created_at,
    })
}

fn resolve_board_post_id(connection: &Connection, reference: &str) -> anyhow::Result<String> {
    let reference = reference.trim();
    validate_non_empty("post reference", reference)?;
    let short_sequence = reference
        .strip_prefix('P')
        .or_else(|| reference.strip_prefix('p'))
        .filter(|sequence| {
            !sequence.is_empty() && sequence.bytes().all(|byte| byte.is_ascii_digit())
        });
    let resolved = if let Some(sequence) = short_sequence {
        if let Ok(sequence) = sequence.parse::<i64>() {
            connection
                .query_row(
                    "select id from operations.society_post where sequence = ?1",
                    params![sequence],
                    |row| row.get::<_, String>(0),
                )
                .optional()?
        } else {
            None
        }
    } else {
        connection
            .query_row(
                "select id from operations.society_post where id = ?1",
                params![reference],
                |row| row.get::<_, String>(0),
            )
            .optional()?
    };
    resolved.ok_or_else(|| StoreError::NotFound("board post not found".to_string()).into())
}

fn validate_headline(headline: &str) -> anyhow::Result<()> {
    validate_non_empty("post headline", headline)?;
    if headline.trim().chars().count() > 120 {
        return Err(StoreError::Validation(
            "post headline must be at most 120 characters".to_string(),
        )
        .into());
    }
    Ok(())
}

fn validate_board_post_body(body: &BoardPostBody) -> anyhow::Result<()> {
    match body {
        BoardPostBody::Text { text } => {
            validate_non_empty("post text", text)?;
            if text.len() > 100_000 {
                return Err(StoreError::Validation(
                    "post text must be at most 100000 bytes".to_string(),
                )
                .into());
            }
        }
        BoardPostBody::Link { url } => {
            let url = url.trim();
            if url.len() > 2_000 || !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(StoreError::Validation(
                    "post link must be an http or https URL of at most 2000 characters".to_string(),
                )
                .into());
            }
        }
        BoardPostBody::File {
            filename,
            media_type,
            content_base64,
        } => {
            validate_non_empty("file name", filename)?;
            validate_non_empty("file media type", media_type)?;
            validate_non_empty("file content", content_base64)?;
            if filename.trim().chars().count() > 255
                || media_type.trim().len() > 200
                || content_base64.len() > 1_000_000
            {
                return Err(StoreError::Validation(
                    "file name, media type, or content exceeds its size limit".to_string(),
                )
                .into());
            }
            base64::engine::general_purpose::STANDARD
                .decode(content_base64)
                .map_err(|_| {
                    StoreError::Validation("file content must be valid base64".to_string())
                })?;
        }
    }
    Ok(())
}

fn validate_positive_obols(amount: i64) -> anyhow::Result<()> {
    if amount <= 0 {
        return Err(StoreError::Validation("obol amount must be positive".to_string()).into());
    }
    if amount > MAX_SAFE_OBOLS {
        return Err(StoreError::Validation(
            "obol amount exceeds the exact wire-format limit".to_string(),
        )
        .into());
    }
    Ok(())
}

fn validate_society_address(address: &str) -> anyhow::Result<()> {
    let address = address.trim();
    if address.len() < 2
        || address.len() > 40
        || !address.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(StoreError::Validation(
            "society address must be 2-40 lowercase letters, numbers, or hyphens".to_string(),
        )
        .into());
    }
    Ok(())
}

fn validate_directory_name(name: &str) -> anyhow::Result<()> {
    validate_non_empty("directory name", name)?;
    if name.trim().chars().count() > 80 {
        return Err(StoreError::Validation(
            "directory name must be at most 80 characters".to_string(),
        )
        .into());
    }
    Ok(())
}

fn normalized_description(description: Option<String>) -> anyhow::Result<Option<String>> {
    let description = description
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if description
        .as_ref()
        .is_some_and(|value| value.contains(['\n', '\r']) || value.chars().count() > 160)
    {
        return Err(StoreError::Validation(
            "directory description must be one line of at most 160 characters".to_string(),
        )
        .into());
    }
    Ok(description)
}

fn validate_board_slug(slug: &str) -> anyhow::Result<()> {
    validate_society_address(slug).map_err(|_| {
        StoreError::Validation(
            "space slug must be 2-40 lowercase letters, numbers, or hyphens".to_string(),
        )
        .into()
    })
}

fn migrate_database(
    name: &str,
    path: &Path,
    migrations: &[Migration],
) -> anyhow::Result<BTreeSet<i64>> {
    let mut connection = Connection::open(path)
        .with_context(|| format!("failed to open {name} sqlite database {}", path.display()))?;

    configure_connection(&connection)?;
    connection.execute_batch(
        "create table if not exists schema_migrations (
            version integer primary key,
            name text not null,
            applied_at text not null default current_timestamp
        );",
    )?;

    let applied_versions = {
        let mut statement = connection.prepare("select version from schema_migrations")?;
        let versions = statement.query_map([], |row| row.get::<_, i64>(0))?;
        versions.collect::<Result<BTreeSet<_>, _>>()?
    };

    let transaction = connection.transaction()?;
    let mut newly_applied = BTreeSet::new();
    for migration in migrations {
        if applied_versions.contains(&migration.version) {
            continue;
        }

        transaction
            .execute_batch(migration.sql)
            .with_context(|| format!("failed to apply {name} migration {}", migration.name))?;
        transaction.execute(
            "insert into schema_migrations (version, name) values (?1, ?2)",
            params![migration.version, migration.name],
        )?;
        newly_applied.insert(migration.version);
    }
    transaction.commit()?;

    Ok(newly_applied)
}

fn configure_connection(connection: &Connection) -> anyhow::Result<()> {
    connection.busy_timeout(SQLITE_BUSY_TIMEOUT)?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

const CORE_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_core_schema",
        sql: include_str!("../migrations/core/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "artifact_metadata",
        sql: include_str!("../migrations/core/0002_artifact_metadata.sql"),
    },
    Migration {
        version: 3,
        name: "read_models_and_indexes",
        sql: include_str!("../migrations/core/0003_read_models_and_indexes.sql"),
    },
    Migration {
        version: 4,
        name: "artifact_projection_indexes",
        sql: include_str!("../migrations/core/0004_artifact_projection_indexes.sql"),
    },
    Migration {
        version: 5,
        name: "stance_evidence",
        sql: include_str!("../migrations/core/0005_stance_evidence.sql"),
    },
    Migration {
        version: 6,
        name: "condition_contracts",
        sql: include_str!("../migrations/core/0006_condition_contracts.sql"),
    },
    Migration {
        version: 7,
        name: "society_identity",
        sql: include_str!("../migrations/core/0007_society_identity.sql"),
    },
];

const EVENTS_MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "initial_events_schema",
    sql: include_str!("../migrations/events/0001_initial.sql"),
}];

const OPERATIONS_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial_operations_schema",
        sql: include_str!("../migrations/operations/0001_initial.sql"),
    },
    Migration {
        version: 2,
        name: "worker_job_target",
        sql: include_str!("../migrations/operations/0002_worker_job_target.sql"),
    },
    Migration {
        version: 3,
        name: "worker_attempt_lifecycle",
        sql: include_str!("../migrations/operations/0003_worker_attempt_lifecycle.sql"),
    },
    Migration {
        version: 4,
        name: "attention_entity_kind",
        sql: include_str!("../migrations/operations/0004_attention_entity_kind.sql"),
    },
    Migration {
        version: 5,
        name: "lesswrong_fetch_idempotency",
        sql: include_str!("../migrations/operations/0005_lesswrong_fetch_idempotency.sql"),
    },
    Migration {
        version: 6,
        name: "auth",
        sql: include_str!("../migrations/operations/0006_auth.sql"),
    },
    Migration {
        version: 7,
        name: "incremental_synthesis_costs",
        sql: include_str!("../migrations/operations/0007_incremental_synthesis_costs.sql"),
    },
    Migration {
        version: 8,
        name: "lesswrong_refresh",
        sql: include_str!("../migrations/operations/0008_lesswrong_refresh.sql"),
    },
    Migration {
        version: 9,
        name: "public_objection",
        sql: include_str!("../migrations/operations/0009_public_objection.sql"),
    },
    Migration {
        version: 10,
        name: "worker_job_effect",
        sql: include_str!("../migrations/operations/0010_worker_job_effect.sql"),
    },
    Migration {
        version: 11,
        name: "refresh_schedule",
        sql: include_str!("../migrations/operations/0011_refresh_schedule.sql"),
    },
    Migration {
        version: 12,
        name: "society_bank_board",
        sql: include_str!("../migrations/operations/0012_society_bank_board.sql"),
    },
    Migration {
        version: 13,
        name: "society_usage",
        sql: include_str!("../migrations/operations/0013_society_usage.sql"),
    },
    Migration {
        version: 14,
        name: "robinson",
        sql: include_str!("../migrations/operations/0014_robinson.sql"),
    },
    Migration {
        version: 15,
        name: "society_attention_epochs_presence",
        sql: include_str!("../migrations/operations/0015_society_attention_epochs_presence.sql"),
    },
    Migration {
        version: 16,
        name: "society_wake_line",
        sql: include_str!("../migrations/operations/0016_society_wake_line.sql"),
    },
    Migration {
        version: 17,
        name: "experiment_injections",
        sql: include_str!("../migrations/operations/0017_experiment_injections.sql"),
    },
    Migration {
        version: 18,
        name: "wake_settlement_integrity",
        sql: include_str!("../migrations/operations/0018_wake_settlement_integrity.sql"),
    },
];

fn read_session_model(
    connection: &Connection,
    thread_id: &str,
) -> anyhow::Result<SessionReadModel> {
    let (id, title, state, anchor_block_id, quote, stance) = connection
        .query_row(
            "select id, title, state, anchor_block_id, quote, stance
             from operations.thread
             where id = ?1",
            params![thread_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?
                        .unwrap_or_else(|| "Session".to_string()),
                    row.get::<_, String>(2)?,
                    row.get::<_, Option<String>>(3)?
                        .unwrap_or_else(|| "doc-root".to_string()),
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("session not found".to_string()))?;

    let mut statement = connection.prepare(
        "select id, role, content
         from operations.message
         where thread_id = ?1
         order by created_at, id",
    )?;
    let rows = statement.query_map(params![thread_id], |row| {
        Ok(SessionTurn {
            id: row.get(0)?,
            who: match row.get::<_, String>(1)?.as_str() {
                "agent" => TurnAuthor::Agent,
                _ => TurnAuthor::You,
            },
            text: row.get(2)?,
        })
    })?;
    let turns = rows.collect::<Result<Vec<_>, _>>()?;

    Ok(SessionReadModel {
        id,
        anchor: SessionAnchor {
            block_id: anchor_block_id,
            title,
            quote,
        },
        turns,
        stance: stance.as_deref().and_then(parse_stance),
        state,
    })
}

fn read_doc_model(connection: &Connection, space_id: &str) -> anyhow::Result<SynthesisDoc> {
    connection
        .query_row(
            "select space_id, title, version, compiled_at, nodes_json
             from synthesis_doc
             where space_id = ?1",
            params![space_id],
            |row| {
                let nodes_json = row.get::<_, String>(4)?;
                Ok(SynthesisDoc {
                    space_id: row.get(0)?,
                    title: row.get(1)?,
                    version: row.get(2)?,
                    compiled_at: persisted_timestamp(row.get(3)?),
                    nodes: serde_json::from_str(&nodes_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("synthesis document not found".to_string()).into())
}

fn read_report_artifact(
    connection: &Connection,
    artifact_id: &str,
) -> anyhow::Result<SynthesisReport> {
    connection
        .query_row(
            "select content, metadata_json from artifact
             where id = ?1 and kind = 'derived'
               and json_extract(metadata_json, '$.artifactType') = 'synthesisReport'",
            params![artifact_id],
            |row| {
                let content = row.get::<_, String>(0)?;
                let mut metadata = parse_json(row.get::<_, String>(1)?);
                if metadata.pointer("/createdAt").and_then(Value::as_str) == Some("just now") {
                    metadata["createdAt"] = Value::String(String::new());
                }
                Ok(SynthesisReport {
                    artifact_id: artifact_id.to_string(),
                    space_id: string_at(&metadata, "/spaceId", ""),
                    version: metadata
                        .pointer("/reportVersion")
                        .and_then(Value::as_i64)
                        .unwrap_or(1),
                    content,
                    mode: match string_at(&metadata, "/mode", "full").as_str() {
                        "selective" => SynthesisMode::Selective,
                        _ => SynthesisMode::Full,
                    },
                    routing_reason: optional_string_at(&metadata, "/routingReason"),
                    created_at: string_at(&metadata, "/createdAt", ""),
                    provenance_anchors: metadata
                        .get("provenanceAnchors")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default(),
                })
            },
        )
        .optional()?
        .ok_or_else(|| {
            StoreError::NotFound(format!("synthesis effect artifact {artifact_id} not found"))
                .into()
        })
}

fn read_report_model(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<Option<SynthesisReport>> {
    connection
        .query_row(
            "select id, content, metadata_json
             from artifact
             where kind = 'derived'
               and json_extract(metadata_json, '$.artifactType') = 'synthesisReport'
               and json_extract(metadata_json, '$.spaceId') = ?1
             order by cast(json_extract(metadata_json, '$.reportVersion') as integer) desc
             limit 1",
            params![space_id],
            |row| {
                let artifact_id = row.get::<_, String>(0)?;
                let content = row.get::<_, String>(1)?;
                let mut metadata = parse_json(row.get::<_, String>(2)?);
                if metadata.pointer("/createdAt").and_then(Value::as_str) == Some("just now") {
                    metadata["createdAt"] = Value::String(String::new());
                }
                Ok(SynthesisReport {
                    artifact_id,
                    space_id: space_id.to_string(),
                    version: metadata
                        .pointer("/reportVersion")
                        .and_then(Value::as_i64)
                        .unwrap_or(1),
                    content,
                    mode: match string_at(&metadata, "/mode", "full").as_str() {
                        "selective" => SynthesisMode::Selective,
                        _ => SynthesisMode::Full,
                    },
                    routing_reason: optional_string_at(&metadata, "/routingReason"),
                    created_at: string_at(&metadata, "/createdAt", ""),
                    provenance_anchors: metadata
                        .get("provenanceAnchors")
                        .cloned()
                        .and_then(|value| serde_json::from_value(value).ok())
                        .unwrap_or_default(),
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn read_report_history_model(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<Vec<ReportVersion>> {
    ensure_space_exists(connection, space_id)?;
    let mut statement = connection.prepare(
        "select id, content, metadata_json
         from artifact
         where kind = 'derived'
           and json_extract(metadata_json, '$.artifactType') = 'synthesisReport'
           and json_extract(metadata_json, '$.spaceId') = ?1
         order by cast(json_extract(metadata_json, '$.reportVersion') as integer), rowid",
    )?;
    let rows = statement.query_map(params![space_id], |row| {
        let mut metadata = parse_json(row.get::<_, String>(2)?);
        if metadata.pointer("/createdAt").and_then(Value::as_str) == Some("just now") {
            metadata["createdAt"] = Value::String(String::new());
        }
        Ok((
            SynthesisReport {
                artifact_id: row.get(0)?,
                space_id: space_id.to_string(),
                version: metadata
                    .pointer("/reportVersion")
                    .and_then(Value::as_i64)
                    .unwrap_or(1),
                content: row.get(1)?,
                mode: match string_at(&metadata, "/mode", "full").as_str() {
                    "selective" => SynthesisMode::Selective,
                    _ => SynthesisMode::Full,
                },
                routing_reason: optional_string_at(&metadata, "/routingReason"),
                created_at: string_at(&metadata, "/createdAt", ""),
                provenance_anchors: metadata
                    .get("provenanceAnchors")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default(),
            },
            metadata
                .get("causedBy")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default(),
        ))
    })?;
    let reports = rows.collect::<Result<Vec<_>, _>>()?;
    let mut history = Vec::with_capacity(reports.len());
    for (index, (report, caused_by)) in reports.into_iter().enumerate() {
        let diff_from_previous = history
            .last()
            .map(|previous: &ReportVersion| line_diff(&previous.report.content, &report.content))
            .unwrap_or_default();
        debug_assert_eq!(index + 1, report.version as usize);
        history.push(ReportVersion {
            report,
            caused_by,
            diff_from_previous,
        });
    }
    Ok(history)
}

fn read_feed_cause(
    connection: &Connection,
    event_id: &str,
    kind: &str,
    target_id: &str,
    payload: &Value,
) -> anyhow::Result<ReportCause> {
    let source_id = optional_string_at(payload, "/sourceArtifactId")
        .or_else(|| optional_string_at(payload, "/evidenceArtifactId"));
    let source = source_id
        .as_deref()
        .map(|artifact_id| read_source_link(connection, artifact_id))
        .transpose()?;
    let (objection_anchor, unverified_display_name) = if kind == "objection_submitted" {
        let metadata = source_id
            .as_deref()
            .map(|artifact_id| {
                connection.query_row(
                    "select metadata_json from artifact where id = ?1",
                    params![artifact_id],
                    |row| Ok(parse_json(row.get::<_, String>(0)?)),
                )
            })
            .transpose()?
            .unwrap_or_else(|| json!({}));
        (
            metadata
                .get("anchor")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok()),
            optional_string_at(&metadata, "/displayName"),
        )
    } else {
        (None, None)
    };
    let label = match kind {
        "lesswrong_landscape_delta_accepted" => match string_at(payload, "/action", "new").as_str()
        {
            "attach" => "Accepted new evidence or holders for an existing position",
            _ => "Accepted a new imported position",
        },
        "ingest_entity_proposed" => "Accepted a new input",
        "entity_create_requested" => "Accepted a new position",
        "session_commit" => "Accepted a review decision",
        "conditional_stance_requested" => "Accepted a condition on a stance",
        "objection_submitted" => "Accepted a public objection",
        "entity_text_updated" => "Updated an accepted position",
        "entity_state_changed" => "Changed an accepted position's state",
        _ => "Accepted change",
    }
    .to_string();
    let state_change = (kind == "entity_state_changed").then(|| EntityStateChange {
        old_state: string_at(payload, "/oldState", "unknown"),
        new_state: optional_string_at(payload, "/newState")
            .or_else(|| optional_string_at(payload, "/state"))
            .unwrap_or_else(|| "unknown".to_string()),
    });
    let author_name = source_id
        .as_deref()
        .map(|artifact_id| {
            connection
                .query_row(
                    "select coalesce(json_extract(actor.keys_json, '$.username'),
                                     json_extract(actor.keys_json, '$.displayName'),
                                     submitted.value)
                     from artifact, json_each(artifact.metadata_json, '$.submittedBy') submitted
                     left join actor on actor.id = submitted.value
                     where artifact.id = ?1",
                    params![artifact_id],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .transpose()?
        .flatten();
    let _ = target_id;
    Ok(ReportCause {
        event_id: event_id.to_string(),
        kind: kind.to_string(),
        label,
        source,
        author_name,
        unverified_display_name,
        objection_anchor,
        state_change,
        input_text: None,
        attributed_claim: None,
        change_classification: None,
        classification_reason: None,
    })
}

fn report_causes_since_previous(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<Vec<ReportCause>> {
    let previous_artifact_id =
        read_report_model(connection, space_id)?.map(|report| report.artifact_id);
    let previous_event_rowid = previous_artifact_id
        .as_deref()
        .map_or(Ok(0), |artifact_id| {
            connection.query_row(
                "select coalesce(max(rowid), 0) from events.event
             where kind in ('synthesis_report_written', 'report_written') and target_id = ?1",
                params![artifact_id],
                |row| row.get(0),
            )
        })?;
    let mut statement = connection.prepare(
        "select accepted.id, accepted.kind, accepted.target_id, accepted.payload_json
         from events.event accepted
         where accepted.decision_state = 'decided'
           and (accepted.rowid > ?1 or exists (
               select 1 from operations.attention_item review
               where review.target_id = accepted.id and review.state = 'resolved'
                 and json_extract(review.payload_json, '$.decidedAfterEventRowid') >= ?1
           ))
           and accepted.kind in ('lesswrong_landscape_delta_accepted', 'ingest_entity_proposed',
                                 'entity_create_requested', 'session_commit', 'objection_submitted',
                                 'entity_text_updated', 'entity_state_changed')
           and (json_extract(accepted.payload_json, '$.spaceId') = ?2
                or accepted.target_id in (select id from entity where space_id = ?2))
         order by accepted.rowid",
    )?;
    let rows = statement.query_map(params![previous_event_rowid, space_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            parse_json(row.get::<_, String>(3)?),
        ))
    })?;
    rows.map(|row| {
        let (event_id, kind, target_id, payload) = row?;
        let source_id = optional_string_at(&payload, "/sourceArtifactId");
        let source = source_id
            .as_deref()
            .map(|artifact_id| read_source_link(connection, artifact_id))
            .transpose()?;
        let (objection_anchor, unverified_display_name) = if kind == "objection_submitted" {
            let metadata = source_id
                .as_deref()
                .map(|artifact_id| {
                    connection.query_row(
                        "select metadata_json from artifact where id = ?1",
                        params![artifact_id],
                        |row| Ok(parse_json(row.get::<_, String>(0)?)),
                    )
                })
                .transpose()?
                .unwrap_or_else(|| json!({}));
            (
                metadata
                    .get("anchor")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
                optional_string_at(&metadata, "/displayName"),
            )
        } else {
            (None, None)
        };
        let label = match kind.as_str() {
            "lesswrong_landscape_delta_accepted" => {
                match string_at(&payload, "/action", "new").as_str() {
                    "attach" => "Accepted new evidence or holders for an existing position",
                    _ => "Accepted a new imported position",
                }
            }
            "ingest_entity_proposed" => "Accepted a new input",
            "entity_create_requested" => "Accepted a new position",
            "session_commit" => "Accepted a review decision",
            "objection_submitted" => "Accepted a public objection",
            "entity_text_updated" => "Updated an accepted position",
            "entity_state_changed" => "Changed an accepted position's state",
            _ => "Accepted change",
        }
        .to_string();
        let input_text = match kind.as_str() {
            "lesswrong_landscape_delta_accepted" => Some(format!(
                "Position: {}\nFinding: {}\nMerge reasoning: {}",
                string_at(&payload, "/targetPositionText", ""),
                string_at(&payload, "/inputText", ""),
                string_at(&payload, "/reasoning", ""),
            )),
            "ingest_entity_proposed" => source_id
                .as_deref()
                .map(|artifact_id| {
                    connection.query_row(
                        "select content from artifact where id = ?1",
                        params![artifact_id],
                        |row| row.get::<_, String>(0),
                    )
                })
                .transpose()?,
            "entity_create_requested" | "entity_text_updated" | "entity_state_changed" => {
                connection
                    .query_row(
                        "select content from entity where id = ?1",
                        params![target_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()?
            }
            "session_commit" => optional_string_at(&payload, "/summary"),
            "objection_submitted" => source_id
                .as_deref()
                .map(|artifact_id| {
                    connection.query_row(
                        "select content, metadata_json from artifact where id = ?1",
                        params![artifact_id],
                        |row| {
                            let objection = row.get::<_, String>(0)?;
                            let metadata = parse_json(row.get::<_, String>(1)?);
                            let anchor = metadata
                                .get("anchor")
                                .cloned()
                                .and_then(|value| serde_json::from_value::<ObjectionAnchor>(value).ok());
                            Ok(match anchor {
                                Some(anchor) => format!(
                                    "Public objection anchored to original report v{}, characters {}-{}.\nQuoted span: \"{}\"\nObjection: {}",
                                    anchor.report_version,
                                    anchor.start,
                                    anchor.end,
                                    anchor.quote,
                                    objection,
                                ),
                                None => format!("Public objection: {objection}"),
                            })
                        },
                    )
                })
                .transpose()?,
            _ => None,
        };
        let state_change = (kind == "entity_state_changed").then(|| EntityStateChange {
            old_state: string_at(&payload, "/oldState", "unknown"),
            new_state: optional_string_at(&payload, "/newState")
                .or_else(|| optional_string_at(&payload, "/state"))
                .unwrap_or_else(|| "unknown".to_string()),
        });
        let attributed_claim = (kind == "ingest_entity_proposed")
            .then(|| {
                connection
                    .query_row(
                        "select content from entity where id = ?1",
                        params![target_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
            })
            .transpose()?
            .flatten();
        let author_name = source_id
            .as_deref()
            .map(|artifact_id| {
                connection
                    .query_row(
                        "select coalesce(json_extract(actor.keys_json, '$.username'),
                                         json_extract(actor.keys_json, '$.displayName'),
                                         submitted.value)
                         from artifact, json_each(artifact.metadata_json, '$.submittedBy') submitted
                         left join actor on actor.id = submitted.value
                         where artifact.id = ?1",
                        params![artifact_id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
            })
            .transpose()?
            .flatten();
        Ok(ReportCause {
            event_id,
            kind,
            label,
            source,
            author_name,
            unverified_display_name,
            objection_anchor,
            state_change,
            input_text,
            attributed_claim,
            change_classification: None,
            classification_reason: None,
        })
    })
    .collect()
}

fn derive_report_provenance(
    connection: &Connection,
    space_id: &str,
    content: &str,
) -> anyhow::Result<Vec<ReportProvenanceAnchor>> {
    let mut entity_statement = connection.prepare(
        "select entity.id, entity.content
         from entity
         where entity.space_id = ?1
           and exists (select 1 from relation
                       where relation.from_id = entity.id and relation.kind = 'derived_from')
         order by entity.rowid, entity.id",
    )?;
    let entities = entity_statement
        .query_map(params![space_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let Some(source_record_start) = content.find("## Source record") else {
        return Ok(vec![]);
    };
    let source_record_end = content[source_record_start..]
        .find("## Synthesis interpretation")
        .map(|offset| source_record_start + offset)
        .unwrap_or(content.len());
    let source_record = &content[source_record_start..source_record_end];
    let mut anchors = Vec::new();
    for (entity_id, entity_text) in entities {
        if entity_text.is_empty() {
            continue;
        }
        let mut source_statement = connection.prepare(
            "select distinct relation.to_id
             from relation
             join artifact on artifact.id = relation.to_id
             where relation.from_id = ?1 and relation.kind = 'derived_from'
               and artifact.kind in ('source', 'annotation')
             order by relation.to_id",
        )?;
        let source_ids = source_statement
            .query_map(params![entity_id], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let sources = source_ids
            .iter()
            .map(|artifact_id| read_source_link(connection, artifact_id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        if sources.is_empty() {
            continue;
        }
        let matches = source_record
            .match_indices(&entity_text)
            .filter_map(|(relative_start, _)| {
                let byte_start = source_record_start + relative_start;
                let line_start = content[..byte_start]
                    .rfind('\n')
                    .map_or(0, |index| index + 1);
                is_holder_position_prefix(&content[line_start..byte_start]).then_some(byte_start)
            })
            .collect::<Vec<_>>();
        let [byte_start] = matches.as_slice() else {
            continue;
        };
        let start = content[..*byte_start].chars().count();
        let end = start + entity_text.chars().count();
        anchors.push(ReportProvenanceAnchor {
            id: String::new(),
            start,
            end,
            entity_id,
            sources,
        });
    }
    anchors.sort_by(|left, right| {
        (left.start, left.end, left.entity_id.as_str()).cmp(&(
            right.start,
            right.end,
            right.entity_id.as_str(),
        ))
    });
    let ambiguous = anchors
        .iter()
        .enumerate()
        .filter_map(|(index, anchor)| {
            anchors
                .iter()
                .enumerate()
                .any(|(other_index, other)| {
                    index != other_index && anchor.start < other.end && other.start < anchor.end
                })
                .then_some(index)
        })
        .collect::<std::collections::HashSet<_>>();
    anchors = anchors
        .into_iter()
        .enumerate()
        .filter_map(|(index, anchor)| (!ambiguous.contains(&index)).then_some(anchor))
        .collect();
    for (index, anchor) in anchors.iter_mut().enumerate() {
        anchor.id = format!("prov-{:04}", index + 1);
    }
    Ok(anchors)
}

fn is_holder_position_prefix(prefix: &str) -> bool {
    let Some(holder) = prefix
        .strip_prefix("- **")
        .and_then(|value| value.strip_suffix(":** "))
    else {
        return false;
    };
    !holder.is_empty() && !holder.contains("**") && !holder.contains('\n')
}

fn read_source_link(connection: &Connection, artifact_id: &str) -> anyhow::Result<SourceLink> {
    connection
        .query_row(
            "select content, metadata_json from artifact where id = ?1",
            params![artifact_id],
            |row| {
                let content = row.get::<_, String>(0)?;
                let metadata = parse_json(row.get::<_, String>(1)?);
                let label = optional_string_at(&metadata, "/title")
                    .or_else(|| optional_string_at(&metadata, "/sourceKind"))
                    .unwrap_or_else(|| content.chars().take(72).collect());
                Ok(SourceLink {
                    artifact_id: artifact_id.to_string(),
                    label,
                    url: optional_string_at(&metadata, "/sourceUrl"),
                    authored_at: optional_string_at(&metadata, "/postedAt"),
                })
            },
        )
        .map_err(Into::into)
}

fn diff_hunks(diff: &[DiffLine]) -> Vec<Vec<DiffLine>> {
    const CONTEXT_LINES: usize = 2;
    let changed = diff
        .iter()
        .enumerate()
        .filter_map(|(index, line)| (line.kind != DiffKind::Context).then_some(index))
        .collect::<Vec<_>>();
    let Some(&first) = changed.first() else {
        return vec![];
    };
    let mut ranges = Vec::new();
    let mut start = first.saturating_sub(CONTEXT_LINES);
    let mut end = (first + CONTEXT_LINES + 1).min(diff.len());
    for &index in changed.iter().skip(1) {
        let next_start = index.saturating_sub(CONTEXT_LINES);
        let next_end = (index + CONTEXT_LINES + 1).min(diff.len());
        if next_start <= end {
            end = end.max(next_end);
        } else {
            ranges.push((start, end));
            start = next_start;
            end = next_end;
        }
    }
    ranges.push((start, end));
    ranges
        .into_iter()
        .map(|(start, end)| diff[start..end].to_vec())
        .collect()
}

fn line_diff(before: &str, after: &str) -> Vec<DiffLine> {
    let before = before.lines().collect::<Vec<_>>();
    let after = after.lines().collect::<Vec<_>>();
    let mut lengths = vec![vec![0usize; after.len() + 1]; before.len() + 1];
    for left in (0..before.len()).rev() {
        for right in (0..after.len()).rev() {
            lengths[left][right] = if before[left] == after[right] {
                lengths[left + 1][right + 1] + 1
            } else {
                lengths[left + 1][right].max(lengths[left][right + 1])
            };
        }
    }
    let (mut left, mut right) = (0, 0);
    let mut diff = Vec::new();
    while left < before.len() || right < after.len() {
        if left < before.len() && right < after.len() && before[left] == after[right] {
            diff.push(DiffLine {
                kind: DiffKind::Context,
                text: before[left].to_string(),
            });
            left += 1;
            right += 1;
        } else if right < after.len()
            && (left == before.len() || lengths[left][right + 1] >= lengths[left + 1][right])
        {
            diff.push(DiffLine {
                kind: DiffKind::Added,
                text: after[right].to_string(),
            });
            right += 1;
        } else {
            diff.push(DiffLine {
                kind: DiffKind::Removed,
                text: before[left].to_string(),
            });
            left += 1;
        }
    }
    diff
}

fn read_optional_refresh_schedule_model(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<Option<RefreshSchedule>> {
    connection
        .query_row(
            "select space_id, cadence_seconds, next_due_at, enabled, created_at
             from operations.refresh_schedule where space_id = ?1",
            params![space_id],
            |row| {
                Ok(RefreshSchedule {
                    space_id: row.get(0)?,
                    cadence_seconds: row.get(1)?,
                    next_due_at: row.get(2)?,
                    enabled: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn read_refresh_schedule_model(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<RefreshSchedule> {
    read_optional_refresh_schedule_model(connection, space_id)?
        .ok_or_else(|| StoreError::NotFound("refresh schedule not found".to_string()).into())
}

fn write_doc_model(
    transaction: &rusqlite::Transaction<'_>,
    doc: &SynthesisDoc,
) -> anyhow::Result<()> {
    transaction.execute(
        "insert into synthesis_doc (space_id, title, version, compiled_at, nodes_json)
         values (?1, ?2, ?3, ?4, ?5)
         on conflict(space_id) do update set
            title = excluded.title,
            version = excluded.version,
            compiled_at = excluded.compiled_at,
            nodes_json = excluded.nodes_json",
        params![
            doc.space_id,
            doc.title,
            doc.version,
            doc.compiled_at,
            serde_json::to_string(&doc.nodes)?
        ],
    )?;
    Ok(())
}

fn recompile_doc_projection(
    transaction: &rusqlite::Transaction<'_>,
    space_id: &str,
) -> anyhow::Result<SynthesisDoc> {
    let previous = transaction
        .query_row(
            "select space_id, title, version, compiled_at, nodes_json
             from synthesis_doc
             where space_id = ?1",
            params![space_id],
            |row| {
                let nodes_json = row.get::<_, String>(4)?;
                Ok(SynthesisDoc {
                    space_id: row.get(0)?,
                    title: row.get(1)?,
                    version: row.get(2)?,
                    compiled_at: row.get(3)?,
                    nodes: serde_json::from_str(&nodes_json).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Text,
                            Box::new(error),
                        )
                    })?,
                })
            },
        )
        .optional()?;
    let title = transaction
        .query_row(
            "select settings_json from space where id = ?1",
            params![space_id],
            |row| Ok(parse_json(row.get::<_, String>(0)?)),
        )
        .optional()?
        .map(|settings| string_at(&settings, "/title", "Untitled space"))
        .ok_or_else(|| StoreError::NotFound("space not found".to_string()))?;
    let entity_nodes = compile_entity_nodes(transaction, space_id)?;
    let nodes = merge_compiled_entities(previous.as_ref(), entity_nodes);
    let doc = SynthesisDoc {
        space_id: space_id.to_string(),
        title,
        version: previous.as_ref().map(|doc| doc.version + 1).unwrap_or(1),
        compiled_at: now_label(),
        nodes,
    };
    write_doc_model(transaction, &doc)?;
    Ok(doc)
}

fn contract_kind(value: &str) -> ContractKind {
    match value {
        "conditional_stance" => ContractKind::ConditionalStance,
        _ => ContractKind::Falsifier,
    }
}

fn source_link_from_parts(artifact_id: String, content: String, metadata: &Value) -> SourceLink {
    SourceLink {
        artifact_id,
        label: optional_string_at(metadata, "/title")
            .or_else(|| optional_string_at(metadata, "/sourceKind"))
            .or_else(|| optional_string_at(metadata, "/inputKind"))
            .unwrap_or_else(|| content.chars().take(72).collect()),
        url: optional_string_at(metadata, "/sourceUrl"),
        authored_at: optional_string_at(metadata, "/postedAt"),
    }
}

fn read_holder_conditions(
    connection: &Connection,
    entity_id: &str,
    actor_id: &str,
) -> anyhow::Result<Vec<HolderCondition>> {
    let mut statement = connection.prepare(
        "select contract.id, contract.kind, contract.terms, condition.id, condition.content,
                artifact.id, artifact.content, artifact.metadata_json
         from contract
         join entity condition on condition.id = contract.condition_entity_id
         join artifact on artifact.id = contract.evidence_artifact_id
         where contract.target_entity_id = ?1 and contract.actor_id = ?2
           and contract.kind in ('falsifier', 'conditional_stance') and contract.status = 'open'
           and condition.role = 'condition'
         order by contract.rowid",
    )?;
    let rows = statement.query_map(params![entity_id, actor_id], |row| {
        let content = row.get::<_, String>(6)?;
        let metadata = parse_json(row.get::<_, String>(7)?);
        Ok(HolderCondition {
            contract_id: row.get(0)?,
            kind: contract_kind(&row.get::<_, String>(1)?),
            terms: row.get(2)?,
            condition_entity_id: row.get(3)?,
            condition_text: row.get(4)?,
            evidence: source_link_from_parts(row.get(5)?, content, &metadata),
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn read_position_holders(
    connection: &Connection,
    entity_id: &str,
) -> anyhow::Result<Vec<PositionHolder>> {
    let mut statement = connection.prepare(
        "select stance.actor_id,
                coalesce(json_extract(actor.keys_json, '$.displayName'), json_extract(actor.keys_json, '$.username'), stance.actor_id),
                stance.value,
                stance.created_at,
                stance.stated_confidence,
                artifact.id,
                artifact.content,
                artifact.metadata_json
         from stance
         join actor on actor.id = stance.actor_id
         left join artifact on artifact.id = stance.evidence_artifact_id
         where stance.target_id = ?1 and stance.axis = 'endorse'
           and (stance.evidence_artifact_id is not null or stance.actor_id not like 'lw-actor-%')
         order by lower(coalesce(json_extract(actor.keys_json, '$.displayName'), json_extract(actor.keys_json, '$.username'), stance.actor_id)),
                  stance.actor_id,
                  case when stance.evidence_artifact_id is null then 1 else 0 end,
                  julianday(json_extract(artifact.metadata_json, '$.postedAt')) desc,
                  stance.created_at desc,
                  stance.rowid desc",
    )?;
    let rows = statement.query_map(params![entity_id], |row| {
        let artifact_id = row.get::<_, Option<String>>(5)?;
        let content = row.get::<_, Option<String>>(6)?.unwrap_or_default();
        let metadata = parse_json(
            row.get::<_, Option<String>>(7)?
                .unwrap_or_else(|| "{}".to_string()),
        );
        let evidence = artifact_id
            .map(|artifact_id| SourceLink {
                artifact_id,
                label: optional_string_at(&metadata, "/sourceKind")
                    .unwrap_or_else(|| content.chars().take(72).collect()),
                url: optional_string_at(&metadata, "/sourceUrl"),
                authored_at: optional_string_at(&metadata, "/postedAt"),
            })
            .into_iter()
            .collect();
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            HistoricalStance {
                stance: row.get(2)?,
                accepted_at: row.get(3)?,
                stated_confidence: row.get(4)?,
                evidence,
            },
        ))
    })?;
    let stances = rows.collect::<Result<Vec<_>, _>>()?;
    let mut holders = Vec::<PositionHolder>::new();
    for (actor_id, actor_name, stance) in stances {
        if let Some(holder) = holders
            .last_mut()
            .filter(|holder| holder.actor_id == actor_id)
        {
            holder.history.push(stance);
        } else {
            let conditions = read_holder_conditions(connection, entity_id, &actor_id)?;
            holders.push(PositionHolder {
                actor_id,
                actor_name,
                stance: stance.stance,
                accepted_at: stance.accepted_at,
                stated_confidence: stance.stated_confidence,
                evidence: stance.evidence,
                history: Vec::new(),
                conditions,
            });
        }
    }
    Ok(holders)
}

fn compile_entity_nodes(
    transaction: &rusqlite::Transaction<'_>,
    space_id: &str,
) -> anyhow::Result<Vec<DocNode>> {
    let mut statement = transaction.prepare(
        "select id, content, role, state
         from entity
         where space_id = ?1
         order by rowid, id",
    )?;
    let rows = statement.query_map(params![space_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    let entities = rows.collect::<Result<Vec<_>, _>>()?;
    entities
        .into_iter()
        .map(|(id, text, role, state)| {
            let holders = read_position_holders(transaction, &id)?;
            Ok(entity_node_from_source(id, text, role, state, holders))
        })
        .collect()
}

fn entity_node_from_source(
    id: String,
    text: String,
    role: String,
    state: String,
    holders: Vec<PositionHolder>,
) -> DocNode {
    let state_metadata = serde_json::from_str::<Value>(&state).ok();
    let state_label = state_metadata
        .as_ref()
        .and_then(|value| optional_string_at(value, "/label"))
        .unwrap_or(state);
    DocNode::Entity(EntityNode {
        id,
        role: match role.as_str() {
            "goal" => EntityRole::Goal,
            "action" => EntityRole::Action,
            "condition" => EntityRole::Condition,
            _ => EntityRole::Issue,
        },
        state: state_label,
        text,
        posterior: state_metadata
            .as_ref()
            .and_then(|value| value.pointer("/posterior"))
            .and_then(Value::as_f64),
        markets: state_metadata
            .as_ref()
            .and_then(|value| value.pointer("/markets"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok()),
        half_life_days: state_metadata
            .as_ref()
            .and_then(|value| value.pointer("/halfLifeDays"))
            .and_then(Value::as_i64),
        dissent: state_metadata
            .as_ref()
            .and_then(|value| value.pointer("/dissent"))
            .and_then(Value::as_bool),
        holders,
        neighbors: state_metadata
            .as_ref()
            .and_then(|value| value.pointer("/neighbors"))
            .cloned()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
    })
}

fn merge_compiled_entities(
    previous: Option<&SynthesisDoc>,
    entity_nodes: Vec<DocNode>,
) -> Vec<DocNode> {
    let Some(previous) = previous else {
        return entity_nodes;
    };

    let mut used = BTreeSet::new();
    let mut nodes = Vec::new();
    for node in &previous.nodes {
        match node {
            DocNode::Entity(entity) => {
                let id = &entity.id;
                if let Some(compiled) = entity_nodes
                    .iter()
                    .find(|candidate| node_id(candidate) == id)
                {
                    used.insert(id.clone());
                    nodes.push(compiled.clone());
                }
            }
            _ => nodes.push(node.clone()),
        }
    }

    for node in entity_nodes {
        if !used.contains(node_id(&node)) {
            nodes.push(node);
        }
    }

    nodes
}

fn looks_like_stated_confidence(value: &str) -> bool {
    let value = value.to_lowercase();
    value.contains('%')
        || [
            "confiden",
            "credence",
            "probab",
            "certain",
            "uncertain",
            "weak",
            "strong",
            "tentative",
            "likely",
            "unlikely",
        ]
        .iter()
        .any(|qualifier| value.contains(qualifier))
}

fn validate_authored_timestamp(
    connection: &Connection,
    label: &str,
    timestamp: &str,
) -> anyhow::Result<()> {
    validate_non_empty(label, timestamp)?;
    let parsed = connection.query_row("select julianday(?1)", params![timestamp], |row| {
        row.get::<_, Option<f64>>(0)
    })?;
    if parsed.is_none() || !timestamp.contains('T') {
        return Err(
            StoreError::Validation(format!("{label} must be an ISO-8601 timestamp")).into(),
        );
    }
    Ok(())
}

fn ensure_space_exists(connection: &Connection, space_id: &str) -> anyhow::Result<()> {
    let exists = connection
        .query_row(
            "select 1 from space where id = ?1",
            params![space_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !exists {
        return Err(StoreError::NotFound("space not found".to_string()).into());
    }
    Ok(())
}

fn space_title(connection: &Connection, space_id: &str) -> anyhow::Result<String> {
    connection
        .query_row(
            "select settings_json from space where id = ?1",
            params![space_id],
            |row| Ok(parse_json(row.get::<_, String>(0)?)),
        )
        .optional()?
        .map(|settings| string_at(&settings, "/title", "Untitled space"))
        .ok_or_else(|| StoreError::NotFound("space not found".to_string()).into())
}

fn entity_space_and_role(
    connection: &Connection,
    entity_id: &str,
) -> anyhow::Result<(String, String)> {
    connection
        .query_row(
            "select space_id, role from entity where id = ?1",
            params![entity_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("entity not found".to_string()).into())
}

fn initial_entity_state(role: &EntityRole) -> &'static str {
    match role {
        EntityRole::Issue | EntityRole::Condition => "open",
        EntityRole::Goal => "proposed",
        EntityRole::Action => "todo",
    }
}

fn validate_ontology_role(role: &str) -> anyhow::Result<()> {
    match role {
        "issue" | "goal" | "action" | "condition" => Ok(()),
        _ => Err(StoreError::Validation(format!("unsupported entity role: {role}")).into()),
    }
}

fn validate_entity_state(role: &str, state: &str) -> anyhow::Result<()> {
    let valid = match role {
        "issue" => matches!(state, "open" | "resolved"),
        "goal" => matches!(state, "proposed" | "adopted" | "achieved" | "abandoned"),
        "action" => matches!(state, "todo" | "doing" | "done" | "cancelled"),
        "condition" => state == "open",
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(StoreError::Validation(format!("state {state} is not valid for {role}")).into())
    }
}

fn excerpt_around_quote(content: &str, quote: &str, radius_chars: usize) -> Option<String> {
    let quote_start = content.find(quote)?;
    let quote_end = quote_start + quote.len();
    let start = content[..quote_start]
        .char_indices()
        .rev()
        .nth(radius_chars.saturating_sub(1))
        .map(|(index, _)| index)
        .unwrap_or(0);
    let end = content[quote_end..]
        .char_indices()
        .nth(radius_chars)
        .map(|(index, _)| quote_end + index)
        .unwrap_or(content.len());
    Some(content[start..end].to_string())
}

fn state_with_label(current_state: &str, label: &str) -> String {
    match serde_json::from_str::<Value>(current_state) {
        Ok(Value::Object(mut object)) => {
            object.insert("label".to_string(), Value::String(label.to_string()));
            Value::Object(object).to_string()
        }
        _ => label.to_string(),
    }
}

fn ensure_thread_owned(
    connection: &Connection,
    thread_id: &str,
    actor_id: &str,
) -> anyhow::Result<()> {
    let exists = connection
        .query_row(
            "select 1 from operations.thread where id = ?1 and owner_id = ?2",
            params![thread_id, actor_id],
            |_| Ok(()),
        )
        .optional()?;
    exists.ok_or_else(|| StoreError::NotFound("session not found".to_string()).into())
}

fn ensure_active_thread_owned(
    connection: &Connection,
    thread_id: &str,
    actor_id: &str,
) -> anyhow::Result<()> {
    ensure_thread_owned(connection, thread_id, actor_id)?;
    let state = connection.query_row(
        "select state from operations.thread where id = ?1",
        params![thread_id],
        |row| row.get::<_, String>(0),
    )?;
    if state != "active" {
        return Err(StoreError::Validation("session is not active".to_string()).into());
    }
    Ok(())
}

fn lesswrong_id(prefix: &str, external_id: &str) -> anyhow::Result<String> {
    validate_non_empty("LessWrong external id", external_id)?;
    if !external_id
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || character == '_' || character == '-')
    {
        return Err(StoreError::Validation("invalid LessWrong external id".to_string()).into());
    }
    Ok(format!("{prefix}-{external_id}"))
}

fn upsert_lesswrong_actor(
    transaction: &rusqlite::Transaction<'_>,
    author: &LessWrongAuthorInput,
) -> anyhow::Result<()> {
    validate_non_empty("author.external_id", &author.external_id)?;
    validate_non_empty("author.display_name", &author.display_name)?;
    let actor_id = lesswrong_id("lw-actor", &author.external_id)?;
    transaction.execute(
        "insert into actor (id, kind, keys_json) values (?1, 'human', ?2)
         on conflict(id) do nothing",
        params![
            actor_id,
            json!({
                "source": "lesswrong",
                "externalId": author.external_id,
                "displayName": author.display_name,
                "slug": author.slug,
            })
            .to_string()
        ],
    )?;
    Ok(())
}

fn ensure_system_actors(transaction: &rusqlite::Transaction<'_>) -> anyhow::Result<()> {
    transaction.execute(
        "insert or ignore into actor (id, kind, keys_json) values (?1, 'human', '{}')",
        params![LOCAL_ACTOR_ID],
    )?;
    transaction.execute(
        "insert or ignore into actor (id, kind, owner_id, keys_json) values (?1, 'agent', ?2, '{}')",
        params![AGENT_ACTOR_ID, LOCAL_ACTOR_ID],
    )?;
    Ok(())
}

fn ensure_artifact_contains(
    connection: &Connection,
    artifact_id: &str,
    quote: &str,
) -> anyhow::Result<()> {
    let content = connection
        .query_row(
            "select content from artifact where id = ?1",
            params![artifact_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .optional()?
        .flatten()
        .ok_or_else(|| StoreError::NotFound("anchor artifact not found".to_string()))?;
    if !content.contains(quote) {
        return Err(StoreError::Validation(
            "anchor quote does not occur in the artifact".to_string(),
        )
        .into());
    }
    Ok(())
}

fn anchor_title(connection: &Connection, anchor_block_id: &str) -> anyhow::Result<String> {
    if anchor_block_id == "doc-root" {
        return Ok(connection
            .query_row(
                "select title from synthesis_doc order by space_id limit 1",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .unwrap_or_else(|| "this space".to_string()));
    }
    for doc in all_docs(connection)? {
        if let Some(node) = doc
            .nodes
            .iter()
            .find(|node| node_id(node) == anchor_block_id)
        {
            return Ok(shorten(&node_text(node), 80));
        }
    }
    Err(StoreError::NotFound("anchor block not found".to_string()).into())
}

fn space_for_anchor(connection: &Connection, anchor_block_id: &str) -> anyhow::Result<String> {
    if anchor_block_id == "doc-root" {
        return connection
            .query_row(
                "select space_id from synthesis_doc order by space_id limit 1",
                [],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::NotFound("space not found".to_string()).into());
    }
    for doc in all_docs(connection)? {
        if doc
            .nodes
            .iter()
            .any(|node| node_id(node) == anchor_block_id)
        {
            return Ok(doc.space_id);
        }
    }
    Err(StoreError::NotFound("anchor block not found".to_string()).into())
}

fn all_docs(connection: &Connection) -> anyhow::Result<Vec<SynthesisDoc>> {
    let mut statement = connection
        .prepare("select space_id, title, version, compiled_at, nodes_json from synthesis_doc")?;
    let rows = statement.query_map([], |row| {
        let nodes_json = row.get::<_, String>(4)?;
        Ok(SynthesisDoc {
            space_id: row.get(0)?,
            title: row.get(1)?,
            version: row.get(2)?,
            compiled_at: persisted_timestamp(row.get(3)?),
            nodes: serde_json::from_str(&nodes_json).map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    4,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn entity_exists(connection: &rusqlite::Transaction<'_>, entity_id: &str) -> anyhow::Result<bool> {
    Ok(connection
        .query_row(
            "select 1 from entity where id = ?1",
            params![entity_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn session_commit_payload(session: &SessionReadModel) -> Value {
    json!({
        "threadId": session.id,
        "anchorBlockId": session.anchor.block_id,
        "anchorTitle": session.anchor.title,
        "quote": session.anchor.quote,
        "stance": session.stance,
        "turns": session.turns,
    })
}

fn review_payload_from_session(session: &SessionReadModel) -> Value {
    let summary = match &session.stance {
        Some(stance) => format!(
            "You {}: \"{}\"",
            stance.as_str(),
            shorten(
                session
                    .anchor
                    .quote
                    .as_deref()
                    .unwrap_or(&session.anchor.title),
                80,
            )
        ),
        None => {
            let noun = if session.turns.len() == 1 {
                "note"
            } else {
                "notes"
            };
            format!(
                "From your session @ \"{}\": {} {} to integrate",
                session.anchor.title,
                session.turns.len(),
                noun
            )
        }
    };
    json!({
        "summary": summary,
        "detail": session.turns.first().map(|turn| turn.text.clone()),
        "fromSession": {
            "anchorBlockId": session.anchor.block_id,
            "anchorTitle": session.anchor.title,
            "quote": session.anchor.quote,
            "stance": session.stance,
        }
    })
}

fn insert_commit_node(
    nodes: &mut Vec<DocNode>,
    anchor_block_id: &str,
    version: i64,
    summary: &str,
) {
    let mark = DocNode::Commit {
        id: format!("n-commit-{version}"),
        label: format!("v{version}"),
        author: "you".to_string(),
        summary: commit_summary(summary),
    };
    let index = nodes
        .iter()
        .position(|node| node_id(node) == anchor_block_id)
        .map(|index| index + 1)
        .unwrap_or(0);
    nodes.insert(index, mark);
}

fn commit_summary(summary: &str) -> String {
    if let Some(rest) = summary.strip_prefix("You ") {
        format!("stance: {rest}")
    } else {
        summary.to_string()
    }
}

fn node_id(node: &DocNode) -> &str {
    match node {
        DocNode::Heading { id, .. } | DocNode::Prose { id, .. } | DocNode::Commit { id, .. } => id,
        DocNode::Entity(entity) => &entity.id,
    }
}

fn node_text(node: &DocNode) -> String {
    match node {
        DocNode::Heading { text, .. } | DocNode::Prose { text, .. } => text.clone(),
        DocNode::Entity(entity) => entity.text.clone(),
        DocNode::Commit {
            label,
            author,
            summary,
            ..
        } => format!("{label} @{author}: {summary}"),
    }
}

fn parse_stance(value: &str) -> Option<Stance> {
    match value {
        "believe" => Some(Stance::Believe),
        "dispute" => Some(Stance::Dispute),
        _ => None,
    }
}

fn projection_at(value: &Value, pointer: &str) -> Projection {
    match value.pointer(pointer).and_then(Value::as_str) {
        Some("desire") => Projection::Desire,
        Some("action") => Projection::Action,
        _ => Projection::Belief,
    }
}

fn string_array_at(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(ToString::to_string)
        .collect()
}

fn string_at(value: &Value, pointer: &str, fallback: &str) -> String {
    optional_string_at(value, pointer).unwrap_or_else(|| fallback.to_string())
}

fn optional_string_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn parse_json(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap_or_else(|_| json!({}))
}

fn validate_non_empty(field: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        return Err(StoreError::Validation(format!("{field} is required")).into());
    }
    Ok(())
}

fn validate_usage_tokens(field: &str, value: i64) -> anyhow::Result<()> {
    if !(0..=MAX_SAFE_OBOLS).contains(&value) {
        return Err(StoreError::Validation(format!(
            "{field} must be a non-negative exact wire-format integer"
        ))
        .into());
    }
    Ok(())
}

fn priced_wake_obols(usage: &SocietyUsageRecord) -> anyhow::Result<i64> {
    let has_usage = usage.input_tokens > 0
        || usage.cached_input_tokens > 0
        || usage.cache_write_input_tokens > 0
        || usage.output_tokens > 0
        || usage.reasoning_output_tokens > 0;
    if !has_usage {
        return Ok(0);
    }
    let exact_cost = usage
        .cost_usd_exact
        .as_deref()
        .ok_or_else(|| StoreError::Validation("wake usage has no exact USD cost".to_string()))?;
    Ok(exact_usd_to_obols(exact_cost)?.max(1))
}

fn validate_exact_usd(value: &str) -> anyhow::Result<f64> {
    exact_usd_to_obols(value)?;
    value.parse::<f64>().map_err(|_| {
        StoreError::Validation("costUsd must be an exact non-negative decimal string".to_string())
            .into()
    })
}

fn exact_usd_to_obols(value: &str) -> anyhow::Result<i64> {
    let invalid = || {
        StoreError::Validation("costUsd must be an exact non-negative decimal string".to_string())
    };
    if value.is_empty() || value.len() > 100 || value.starts_with('+') || value.starts_with('-') {
        return Err(invalid().into());
    }
    let mut parts = value.split('.');
    let dollars = parts.next().unwrap_or_default();
    let fraction = parts.next();
    if parts.next().is_some()
        || dollars.is_empty()
        || !dollars.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|digits| {
            digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(invalid().into());
    }
    let dollars = dollars.parse::<u64>().map_err(|_| invalid())?;
    let fraction = fraction.unwrap_or_default().as_bytes();
    let first_cent = fraction.first().map(|byte| byte - b'0').unwrap_or(0);
    let second_cent = fraction.get(1).map(|byte| byte - b'0').unwrap_or(0);
    let has_subcent = fraction
        .get(2..)
        .is_some_and(|digits| digits.iter().any(|byte| *byte != b'0'));
    let cents = dollars
        .checked_mul(100)
        .and_then(|amount| amount.checked_add(u64::from(first_cent) * 10))
        .and_then(|amount| amount.checked_add(u64::from(second_cent)))
        .and_then(|amount| amount.checked_add(u64::from(has_subcent)))
        .filter(|amount| *amount <= MAX_SAFE_OBOLS as u64)
        .ok_or_else(|| {
            StoreError::Validation("costUsd exceeds the exact wire-format limit".to_string())
        })?;
    Ok(cents as i64)
}

fn new_id(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{now:x}_{counter:x}")
}

fn shorten(value: &str, max_chars: usize) -> String {
    let mut output = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        output.push('…');
    }
    output
}

fn persisted_timestamp(value: String) -> String {
    if value == "just now" {
        String::new()
    } else {
        value
    }
}

fn now_label() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
        duration.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_settlement_migration_preserves_positive_debits() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-wake-settlement-migration-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("operations.sqlite");
        migrate_database("operations", &path, &OPERATIONS_MIGRATIONS[..17]).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "insert into society_wake_debit
                    (actor_id, wake_number, amount, treasury_actor_id, ledger_id, created_at)
                 values ('alice-id', 1, 7, 'treasury-id', 'ledger-id', '2026-08-29T00:00:00Z')",
                [],
            )
            .unwrap();
        drop(connection);

        migrate_database("operations", &path, OPERATIONS_MIGRATIONS).unwrap();
        let connection = Connection::open(&path).unwrap();
        let migrated = connection
            .query_row(
                "select priced_amount, amount, ledger_id from society_wake_debit
                 where actor_id = 'alice-id' and wake_number = 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(migrated, (7, 7, Some("ledger-id".to_string())));
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn society_birth_replays_only_exact_identity_and_key() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-birth-replay-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        store
            .ensure_society_treasury("treasury")
            .expect("treasury should exist");

        let first = store
            .birth_society_agent("alice", "Alice", "stable-key-hash", 100)
            .expect("first birth should commit");
        let replay = store
            .birth_society_agent("alice", "Alice", "stable-key-hash", 100)
            .expect("exact birth retry should replay");
        assert_eq!(replay, first);
        assert!(
            store
                .birth_society_agent("alice", "Alicia", "stable-key-hash", 100)
                .unwrap_err()
                .to_string()
                .contains("different birth data")
        );
        assert!(
            store
                .birth_society_agent("alice", "Alice", "different-key-hash", 100)
                .unwrap_err()
                .to_string()
                .contains("different birth data")
        );
        assert!(
            store
                .birth_society_agent("bob", "Bob", "stable-key-hash", 100)
                .unwrap_err()
                .to_string()
                .contains("already registered to another address")
        );

        let connection = store.open_workspace_connection().unwrap();
        let actor_count: i64 = connection
            .query_row(
                "select count(*) from actor where kind = 'agent'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(actor_count, 1);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wake_debit_prices_current_usage_and_clamps_to_spendable_balance() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-wake-debit-integrity-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        store
            .ensure_society_treasury("treasury")
            .expect("treasury should exist");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 3)
            .expect("Alice should be born");
        store
            .record_society_wake_started(&alice.actor_id, 1, true)
            .expect("a funded wake should start");
        store
            .record_society_usage(RecordSocietyUsageInput {
                agent_address: alice.address.clone(),
                wake_number: Some(1),
                model: "test-model".to_string(),
                input_tokens: 1,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                cost_usd: Some("0.07".to_string()),
            })
            .expect("usage should be immutable wake input");
        store
            .transfer_obols(
                &alice.actor_id,
                TransferObolsInput {
                    recipient_address: "treasury".to_string(),
                    amount: 2,
                },
            )
            .expect("ordinary wake spending should preempt balance");

        let receipt = store
            .debit_wake(&alice.actor_id, "treasury")
            .expect("settlement should clamp instead of failing");
        assert_eq!(receipt.wake_number, 1);
        assert_eq!(receipt.priced_obols, 7);
        assert_eq!(receipt.charged_obols, 1);
        assert_eq!(receipt.ledger_entry.as_ref().unwrap().amount, 1);
        assert_eq!(
            store.debit_wake(&alice.actor_id, "treasury").unwrap(),
            receipt,
            "a positive debit must replay its recorded charge"
        );

        let connection = store.open_workspace_connection().unwrap();
        let debit_rows: i64 = connection
            .query_row(
                "select count(*) from operations.society_wake_debit where actor_id = ?1",
                params![alice.actor_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(debit_rows, 1);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn zero_charge_settlement_is_durable_without_a_ledger_row() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-wake-zero-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let treasury = store
            .ensure_society_treasury("treasury")
            .expect("treasury should exist");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 1)
            .expect("Alice should be born");
        store
            .transfer_obols(
                &alice.actor_id,
                TransferObolsInput {
                    recipient_address: "treasury".to_string(),
                    amount: 1,
                },
            )
            .expect("setup should spend the balance");
        let error = store
            .record_society_wake_started(&alice.actor_id, 1, true)
            .expect_err("backed admission must reject zero balance");
        assert!(error.to_string().contains("insufficient obol balance"));
        store
            .record_society_wake_started(&alice.actor_id, 1, false)
            .expect("independent admission should ignore balance");
        store
            .record_society_usage(RecordSocietyUsageInput {
                agent_address: alice.address.clone(),
                wake_number: Some(1),
                model: "test-model".to_string(),
                input_tokens: 1,
                cached_input_tokens: 0,
                cache_write_input_tokens: 0,
                output_tokens: 0,
                reasoning_output_tokens: 0,
                cost_usd: Some("0.07".to_string()),
            })
            .unwrap();
        let receipt = store.debit_wake(&alice.actor_id, "treasury").unwrap();
        assert_eq!(receipt.priced_obols, 7);
        assert_eq!(receipt.charged_obols, 0);
        assert!(receipt.ledger_entry.is_none());
        store
            .transfer_obols(
                &treasury.actor_id,
                TransferObolsInput {
                    recipient_address: alice.address.clone(),
                    amount: 1,
                },
            )
            .expect("inbound funds should arrive after zero-charge settlement");
        assert_eq!(
            store.debit_wake(&alice.actor_id, "treasury").unwrap(),
            receipt,
            "a durable zero-charge settlement must not charge later funds"
        );

        let connection = store.open_workspace_connection().unwrap();
        let debit_rows: i64 = connection
            .query_row(
                "select count(*) from operations.society_wake_debit where actor_id = ?1",
                params![alice.actor_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(debit_rows, 1);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_lifecycle_write_waits_for_the_current_writer() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-concurrent-lifecycle-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 100)
            .expect("Alice should be born");
        let bob = store
            .birth_society_agent("bob", "Bob", "bob-key-hash", 100)
            .expect("Bob should be born");
        store
            .record_society_wake_started(&alice.actor_id, 1, false)
            .expect("Alice should wake");

        let mut lock_connection = store.open_workspace_connection().unwrap();
        let lock_transaction = lock_connection.transaction().unwrap();
        lock_transaction
            .execute(
                "update operations.society_agent_lifecycle set updated_at = updated_at
                 where actor_id = ?1",
                params![alice.actor_id],
            )
            .unwrap();

        let concurrent_store = store.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let writer = std::thread::spawn(move || {
            ready_tx.send(()).unwrap();
            concurrent_store.record_society_sleep_started(
                &bob.actor_id,
                SleepStartedInput {
                    wake_number: 1,
                    next_wake_at: "2026-08-28T08:00:00Z".to_string(),
                },
            )
        });
        ready_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(250));
        lock_transaction.commit().unwrap();

        let lifecycle = writer
            .join()
            .expect("concurrent writer should not panic")
            .expect("concurrent writer should wait for the lock");
        assert_eq!(lifecycle.phase, SocietyAgentPhase::Sleeping);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_signal_assignment_replays_for_the_same_wake_without_duplicate_events() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-signal-injection-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 100)
            .expect("Alice should be born");
        store
            .record_society_wake_started(&alice.actor_id, 1, false)
            .expect("Alice should start her first wake");
        store
            .record_society_sleep_started(
                &alice.actor_id,
                SleepStartedInput {
                    wake_number: 1,
                    next_wake_at: "2099-01-01T00:00:00Z".to_string(),
                },
            )
            .expect("Alice should sleep");

        let armed = store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::Signal {
                    name: "treatment-start".to_string(),
                },
                recipients: ExperimentInjectionRecipients::Addresses {
                    addresses: vec!["alice".to_string()],
                },
                message: "The treatment begins now.".to_string(),
                make_due: true,
            })
            .expect("injection should arm");
        assert!(
            store
                .emit_experiment_signal("Treatment-start")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.read_experiment_injections().unwrap()[0].state,
            ExperimentInjectionState::Armed
        );

        let fired = store.emit_experiment_signal("treatment-start").unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].state, ExperimentInjectionState::Fired);
        assert!(
            store
                .emit_experiment_signal("treatment-start")
                .unwrap()
                .is_empty()
        );
        let lifecycle = store
            .read_society_lifecycle(&alice.actor_id)
            .unwrap()
            .unwrap();
        assert_ne!(
            lifecycle.next_wake_at.as_deref(),
            Some("2099-01-01T00:00:00Z")
        );

        let pending = read_pending_experiment_injections(
            &store.open_workspace_connection().unwrap(),
            &alice.actor_id,
        )
        .unwrap();
        assert_eq!(pending[0].injection_id, armed.id);

        let wake = store
            .record_society_wake_started(&alice.actor_id, 2, false)
            .expect("the due wake should start");
        assert_eq!(wake.injected_messages, pending);
        assert!(
            read_pending_experiment_injections(
                &store.open_workspace_connection().unwrap(),
                &alice.actor_id,
            )
            .unwrap()
            .is_empty()
        );
        let repeated = store
            .record_society_wake_started(&alice.actor_id, 2, false)
            .expect("a repeated wake-start report should be idempotent");
        assert_eq!(repeated.injected_messages, wake.injected_messages);

        store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::Signal {
                    name: "during-wake".to_string(),
                },
                recipients: ExperimentInjectionRecipients::Addresses {
                    addresses: vec!["alice".to_string()],
                },
                message: "This belongs to the next wake.".to_string(),
                make_due: false,
            })
            .unwrap();
        store.emit_experiment_signal("during-wake").unwrap();
        let repeated = store
            .record_society_wake_started(&alice.actor_id, 2, false)
            .unwrap();
        assert_eq!(repeated.injected_messages, wake.injected_messages);
        assert_eq!(
            read_pending_experiment_injections(
                &store.open_workspace_connection().unwrap(),
                &alice.actor_id,
            )
            .unwrap()
            .len(),
            1
        );
        store
            .record_society_sleep_started(
                &alice.actor_id,
                SleepStartedInput {
                    wake_number: 2,
                    next_wake_at: "2099-01-01T00:00:00Z".to_string(),
                },
            )
            .unwrap();
        let next_wake = store
            .record_society_wake_started(&alice.actor_id, 3, false)
            .unwrap();
        assert_eq!(next_wake.injected_messages.len(), 1);
        assert_eq!(
            next_wake.injected_messages[0].message,
            "This belongs to the next wake."
        );

        let connection = store.open_workspace_connection().unwrap();
        for (kind, expected) in [
            ("experiment_injection_armed", 2),
            ("experiment_injection_fired", 2),
            ("experiment_injection_delivered", 2),
        ] {
            let count: i64 = connection
                .query_row(
                    "select count(*) from events.event where kind = ?1",
                    params![kind],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                count, expected,
                "{kind} should be emitted once per transition"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn due_timestamp_fires_and_assigns_at_the_new_wake_boundary() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-wake-boundary-injection-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 100)
            .expect("Alice should be born");
        store
            .record_society_wake_started(&alice.actor_id, 1, false)
            .expect("Alice should start her first wake");
        store
            .record_society_sleep_started(
                &alice.actor_id,
                SleepStartedInput {
                    wake_number: 1,
                    next_wake_at: "2099-01-01T00:00:00Z".to_string(),
                },
            )
            .expect("Alice should sleep");

        let boundary_injection = store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::At {
                    timestamp: "2000-01-01T00:00:00Z".to_string(),
                },
                recipients: ExperimentInjectionRecipients::Addresses {
                    addresses: vec!["alice".to_string()],
                },
                message: "This wake includes the boundary treatment.".to_string(),
                make_due: false,
            })
            .expect("due timestamp injection should arm");

        // No background due scan runs before this wake starts.
        let wake = store
            .record_society_wake_started(&alice.actor_id, 2, false)
            .expect("the new wake should fire and assign the due timestamp");
        assert_eq!(wake.injected_messages.len(), 1);
        assert_eq!(
            wake.injected_messages[0].injection_id,
            boundary_injection.id
        );

        let after_start = store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::At {
                    timestamp: "2000-01-01T00:00:01Z".to_string(),
                },
                recipients: ExperimentInjectionRecipients::Addresses {
                    addresses: vec!["alice".to_string()],
                },
                message: "This belongs to a later wake.".to_string(),
                make_due: false,
            })
            .expect("a second due timestamp injection should arm");
        let replay = store
            .record_society_wake_started(&alice.actor_id, 2, false)
            .expect("the same wake should replay its original assignment");
        assert_eq!(replay.injected_messages, wake.injected_messages);

        let injections = store.read_experiment_injections().unwrap();
        assert_eq!(
            injections
                .iter()
                .find(|injection| injection.id == boundary_injection.id)
                .unwrap()
                .state,
            ExperimentInjectionState::Fired
        );
        assert_eq!(
            injections
                .iter()
                .find(|injection| injection.id == after_start.id)
                .unwrap()
                .state,
            ExperimentInjectionState::Armed
        );

        let connection = store.open_workspace_connection().unwrap();
        for (kind, expected) in [
            ("experiment_injection_fired", 1),
            ("experiment_injection_delivered", 1),
        ] {
            let count: i64 = connection
                .query_row(
                    "select count(*) from events.event where kind = ?1",
                    params![kind],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, expected, "{kind} should occur exactly once");
        }
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timestamp_injection_snapshots_all_recipients_at_arm_time() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-timestamp-injection-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let alice = store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 100)
            .expect("Alice should be born");
        let armed = store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::At {
                    timestamp: "2000-01-01T00:00:00Z".to_string(),
                },
                recipients: ExperimentInjectionRecipients::All,
                message: "A timestamp treatment.".to_string(),
                make_due: false,
            })
            .expect("injection should arm");
        store
            .birth_society_agent("bob", "Bob", "bob-key-hash", 100)
            .expect("Bob should be born after arming");

        assert_eq!(store.fire_due_experiment_injections().unwrap().len(), 1);
        assert!(store.fire_due_experiment_injections().unwrap().is_empty());
        assert_eq!(
            read_pending_experiment_injections(
                &store.open_workspace_connection().unwrap(),
                &alice.actor_id,
            )
            .unwrap()[0]
                .injection_id,
            armed.id
        );
        let listed = store.read_experiment_injections().unwrap();
        assert_eq!(listed[0].recipients, ExperimentInjectionRecipients::All);
        assert_eq!(listed[0].deliveries.len(), 1);
        assert_eq!(listed[0].deliveries[0].address, "alice");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn timestamp_trigger_requires_rfc3339_and_is_canonicalized_to_utc() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-timestamp-validation-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).unwrap();
        store
            .birth_society_agent("alice", "Alice", "alice-key-hash", 100)
            .unwrap();

        for invalid in [
            "2026-08-29T12:00:00",
            "2026-02-30T12:00:00Z",
            "2026-08-29 12:00:00Z",
        ] {
            let result = store.arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::At {
                    timestamp: invalid.to_string(),
                },
                recipients: ExperimentInjectionRecipients::All,
                message: "Treatment".to_string(),
                make_due: false,
            });
            assert!(result.is_err(), "{invalid} must not parse as RFC 3339");
        }

        let armed = store
            .arm_experiment_injection(ArmExperimentInjectionInput {
                trigger: ExperimentInjectionTrigger::At {
                    timestamp: "2026-08-29T12:34:56+02:00".to_string(),
                },
                recipients: ExperimentInjectionRecipients::All,
                message: "Treatment".to_string(),
                make_due: false,
            })
            .unwrap();
        assert_eq!(
            armed.trigger,
            ExperimentInjectionTrigger::At {
                timestamp: "2026-08-29T10:34:56Z".to_string()
            }
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn weekly_refill_expires_only_the_unused_prior_grant() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-allowance-expiry-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let treasury = store
            .ensure_society_treasury("treasury")
            .expect("treasury should exist");
        let agent = store
            .create_society_agent("alice", "test-key-hash")
            .expect("agent should exist");
        {
            let mut connection = store.open_workspace_connection().unwrap();
            let transaction = connection.transaction().unwrap();
            let ledger_id = append_ledger_entry(
                &transaction,
                "mint",
                LedgerEntryInput {
                    actor_id: None,
                    recipient_id: Some(&agent.actor_id),
                    escrow_id: None,
                    amount: 100,
                    reference_id: Some("allowance:2020-01-06T00:00:00.000Z"),
                    resolution_authority: None,
                },
            )
            .unwrap();
            transaction
                .execute(
                    "insert into operations.society_allowance_grant
                        (actor_id, window_start, window_end, amount, ledger_id, created_at)
                     values (?1, '2020-01-06T00:00:00.000Z', '2020-01-13T00:00:00.000Z',
                             100, ?2, '2020-01-06T00:00:00.000Z')",
                    params![agent.actor_id, ledger_id],
                )
                .unwrap();
            transaction.commit().unwrap();
        }

        let result = store
            .grant_weekly_allowances(100, &treasury.address)
            .expect("refill should succeed");
        assert_eq!(result.granted_agents, vec!["alice"]);
        let bank = store.read_bank_snapshot().unwrap();
        assert_eq!(
            bank.accounts
                .iter()
                .find(|account| account.address == "alice")
                .unwrap()
                .balance,
            100,
            "the unused old allowance must be replaced by the current allowance"
        );
        assert_eq!(
            bank.accounts
                .iter()
                .find(|account| account.address == "treasury")
                .unwrap()
                .balance,
            100
        );
        let connection = store.open_workspace_connection().unwrap();
        let expiry = connection
            .query_row(
                "select granted_amount, spent_amount, expired_amount
                 from operations.society_allowance_expiry where actor_id = ?1",
                params![agent.actor_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(expiry, (100, 0, 100));
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falsifier_quote_must_be_literal_evidence() {
        let quote = "I would change my mind if the audit found a severe incident.";
        let context = ThreadIngestContext {
            space_id: "space".to_string(),
            space_title: "Question".to_string(),
            thread_url: "https://example.com/thread".to_string(),
            title: "Thread".to_string(),
            standing_positions: vec![],
            new_sources: vec![ImportedSource {
                artifact_id: "source".to_string(),
                thread_artifact_id: "thread".to_string(),
                source_kind: "comment".to_string(),
                author_id: "actor".to_string(),
                author_name: "Author".to_string(),
                content: format!("I endorse the position. {quote}"),
                source_url: "https://example.com/comment".to_string(),
                posted_at: "2026-01-01T00:00:00Z".to_string(),
                parent_artifact_id: None,
            }],
        };
        let proposal = |quote: &str| ThreadPositionProposal {
            action: "new".to_string(),
            target_entity_id: None,
            key: "position".to_string(),
            text: "The position".to_string(),
            reasoning: "Explicitly endorsed".to_string(),
            source_artifact_ids: vec!["source".to_string()],
            stances: vec![ThreadStanceProposal {
                actor_id: "actor".to_string(),
                value: "endorse".to_string(),
                evidence_artifact_id: "source".to_string(),
                stated_confidence: None,
                falsifiers: vec![ThreadFalsifierProposal {
                    quote: quote.to_string(),
                    condition: "The audit finds a severe incident".to_string(),
                }],
            }],
        };
        assert!(validate_thread_position_proposals(&context, &[proposal(quote)]).is_ok());
        let paraphrase = "I would reconsider if an audit discovered a bad incident.";
        let error = validate_thread_position_proposals(&context, &[proposal(paraphrase)])
            .unwrap_err()
            .to_string();
        assert!(error.contains("literal source substrings"));
    }

    #[test]
    fn persisted_placeholder_timestamp_is_unknown() {
        assert_eq!(persisted_timestamp("just now".to_string()), "");
        assert!(now_label().ends_with('Z'));
        assert!(now_label().contains('T'));
    }

    #[test]
    fn stance_evidence_migration_rebuilds_projection_and_uses_authored_chronology() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-stance-upgrade-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let paths = StorePaths::from_root(&root);
        std::fs::create_dir_all(&root).unwrap();
        migrate_database("core", &paths.core, &CORE_MIGRATIONS[..4]).unwrap();
        migrate_database("events", &paths.events, EVENTS_MIGRATIONS).unwrap();
        migrate_database("operations", &paths.operations, OPERATIONS_MIGRATIONS).unwrap();
        let connection = Connection::open(&paths.core).unwrap();
        connection.execute_batch(
            "insert into space (id, settings_json) values ('space', '{\"title\":\"Upgrade\"}');
             insert into actor (id, kind, keys_json) values ('lw-actor-a', 'human', '{\"displayName\":\"Author A\"}');
             insert into artifact (id, kind, media_type, content, metadata_json)
             values ('source-old', 'source', 'text/plain', 'Old position',
                     '{\"authorId\":\"lw-actor-a\",\"sourceKind\":\"comment\",\"postedAt\":\"2020-01-01T00:00:00Z\"}');
             insert into entity (id, space_id, content, role, state)
             values ('entity', 'space', 'Position', 'issue', 'open');
             insert into relation (id, from_id, to_id, kind)
             values ('relation-old', 'entity', 'source-old', 'derived_from');
             insert into stance (id, actor_id, target_id, axis, value, created_at)
             values ('stance-old', 'lw-actor-a', 'entity', 'endorse', 'endorse', '2026-01-01T00:00:00Z');
             insert into synthesis_doc (space_id, title, version, compiled_at, nodes_json)
             values ('space', 'Upgrade', 1, '2026-01-01T00:00:00Z',
                     '[{\"kind\":\"entity\",\"id\":\"entity\",\"role\":\"issue\",\"state\":\"open\",\"text\":\"Position\",\"holders\":[{\"actorId\":\"lw-actor-a\",\"actorName\":\"Author A\",\"stance\":\"endorse\",\"evidence\":[{\"artifactId\":\"stale-bag-entry\",\"label\":\"wrong\",\"url\":null}]}],\"neighbors\":[]}]');",
        ).unwrap();
        drop(connection);

        let store = initialize(&root).unwrap();
        let migrated = store.read_doc("space").unwrap();
        let migrated_holder = migrated
            .nodes
            .iter()
            .find_map(|node| match node {
                DocNode::Entity(entity) => entity.holders.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(migrated_holder.evidence[0].artifact_id, "source-old");

        let mut connection = store.open_workspace_connection().unwrap();
        let transaction = connection.transaction().unwrap();
        transaction.execute_batch(
            "insert into artifact (id, kind, media_type, content, metadata_json)
             values ('source-new', 'source', 'text/plain', 'New position',
                     '{\"authorId\":\"lw-actor-a\",\"sourceKind\":\"comment\",\"postedAt\":\"2021-01-01T00:00:00Z\"}');
             insert into relation (id, from_id, to_id, kind)
             values ('relation-new', 'entity', 'source-new', 'derived_from');
             insert into stance
                 (id, actor_id, target_id, axis, value, evidence_artifact_id, created_at)
             values ('stance-new', 'lw-actor-a', 'entity', 'endorse', 'endorse',
                     'source-new', '2022-01-01T00:00:00Z');",
        ).unwrap();
        recompile_doc_projection(&transaction, "space").unwrap();
        transaction.commit().unwrap();
        let current = store.read_doc("space").unwrap();
        let holder = current
            .nodes
            .iter()
            .find_map(|node| match node {
                DocNode::Entity(entity) => entity.holders.first(),
                _ => None,
            })
            .unwrap();
        assert_eq!(holder.evidence[0].artifact_id, "source-new");
        assert_eq!(holder.history[0].evidence[0].artifact_id, "source-old");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn generalized_input_acceptance_and_report_versions_use_existing_primitives() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-slices-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Test synthesis".to_string(),
                    projection: Projection::Belief,
                },
            )
            .expect("space should be created");
        let receipt = store
            .submit_input(
                LOCAL_ACTOR_ID,
                &space.id,
                SubmitInput {
                    kind: InputKind::Document,
                    content: "Keep this source coarse.".to_string(),
                    anchor: None,
                },
            )
            .expect("input should be stored");
        let review = store
            .record_ingest_proposal(
                &receipt.artifact_id,
                CreateEntityInput {
                    role: EntityRole::Issue,
                    text: "The source should remain one issue.".to_string(),
                },
            )
            .expect("ingest should create review work");
        assert_eq!(
            review.source_artifact_id.as_deref(),
            Some(receipt.artifact_id.as_str())
        );
        let doc = store
            .accept_review_item(LOCAL_ACTOR_ID, &review.id)
            .expect("review acceptance should integrate the proposal");
        assert_eq!(
            doc.nodes
                .iter()
                .filter(|node| matches!(node, DocNode::Entity(_)))
                .count(),
            1
        );
        let context = store
            .build_synthesis_context(&space.id)
            .expect("synthesis context should include provenance");
        assert_eq!(context.sources.len(), 1);
        assert_eq!(context.sources[0].artifact_id, receipt.artifact_id);
        assert_eq!(context.causes.len(), 1);
        assert_eq!(
            context.causes[0].author_name.as_deref(),
            Some(LOCAL_ACTOR_ID)
        );
        assert_eq!(
            context.causes[0].attributed_claim.as_deref(),
            Some("The source should remain one issue.")
        );
        assert_eq!(
            context.causes[0]
                .source
                .as_ref()
                .map(|source| source.artifact_id.as_str()),
            Some(receipt.artifact_id.as_str())
        );

        let first = store
            .write_synthesis_report(
                None,
                &space.id,
                SynthesisMode::Full,
                None,
                "# Report\n\n## Source record\n\n- **Local:** The source should remain one issue.\n\n## Synthesis interpretation\n\nThe source should remain one issue.\n\n## Cruxes\n\n- None yet.",
                &context.causes,
            )
            .expect("full report should be written");
        let second = store
            .write_synthesis_report(
                None,
                &space.id,
                SynthesisMode::Selective,
                None,
                "# Report\n\n## Source record\n\n- **Local:** The source should remain one issue.\n\n## Synthesis interpretation\n\nThe source should remain one issue. Targeted addition.\n\n## Cruxes\n\n- None yet.",
                &[],
            )
            .expect("selective report should be written");
        assert_eq!(first.version, 1);
        assert_eq!(second.version, 2);
        assert_eq!(first.provenance_anchors.len(), 1);
        let provenance = &first.provenance_anchors[0];
        assert_eq!(provenance.sources.len(), 1);
        assert_eq!(provenance.sources[0].artifact_id, receipt.artifact_id);
        assert!(provenance.start < first.content.find("## Synthesis interpretation").unwrap());
        assert_eq!(
            first
                .content
                .chars()
                .skip(provenance.start)
                .take(provenance.end - provenance.start)
                .collect::<String>(),
            "The source should remain one issue."
        );
        let connection = store
            .open_workspace_connection()
            .expect("workspace connection should open");
        let derived_from: String = connection
            .query_row(
                "select derived_from from artifact where id = ?1",
                params![second.artifact_id],
                |row| row.get(0),
            )
            .expect("second report should reference its predecessor");
        assert_eq!(derived_from, first.artifact_id);
        let event_count: i64 = connection
            .query_row(
                "select count(*) from events.event where kind = 'synthesis_report_written'",
                [],
                |row| row.get(0),
            )
            .expect("report history should be queryable");
        assert_eq!(event_count, 2);
        drop(connection);
        let history = store
            .read_report_history(&space.id)
            .expect("history should render");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].caused_by.len(), 1);
        assert!(
            history[1]
                .diff_from_previous
                .iter()
                .any(|line| line.kind == DiffKind::Added)
        );

        std::fs::remove_dir_all(root).expect("test data should be removed");
    }

    #[test]
    fn authored_reports_version_content_and_bound_each_versions_causes() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-authored-reports-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).unwrap();
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Human report edits".to_string(),
                    projection: Projection::Belief,
                },
            )
            .unwrap();

        let first_proposal = store
            .propose_entity(
                LOCAL_ACTOR_ID,
                &space.id,
                CreateEntityInput {
                    role: EntityRole::Issue,
                    text: "First accepted position".to_string(),
                },
            )
            .unwrap();
        store
            .accept_review_item(LOCAL_ACTOR_ID, &first_proposal.id)
            .unwrap();
        let first = store
            .write_authored_report(
                LOCAL_ACTOR_ID,
                &space.id,
                WriteReportInput {
                    content: "# Report\n\nFirst accepted position".to_string(),
                },
            )
            .unwrap();

        let second_proposal = store
            .propose_entity(
                LOCAL_ACTOR_ID,
                &space.id,
                CreateEntityInput {
                    role: EntityRole::Issue,
                    text: "Second accepted position".to_string(),
                },
            )
            .unwrap();
        store
            .accept_review_item(LOCAL_ACTOR_ID, &second_proposal.id)
            .unwrap();
        let second = store
            .write_authored_report(
                LOCAL_ACTOR_ID,
                &space.id,
                WriteReportInput {
                    content: "# Report\n\nFirst accepted position\n\nSecond accepted position"
                        .to_string(),
                },
            )
            .unwrap();

        assert_eq!((first.version, second.version), (1, 2));
        let history = store.read_report_history(&space.id).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].caused_by.len(), 1);
        assert_eq!(history[1].caused_by.len(), 1);
        assert_ne!(
            history[0].caused_by[0].event_id,
            history[1].caused_by[0].event_id
        );
        let connection = store.open_workspace_connection().unwrap();
        let event_actors: String = connection
            .query_row(
                "select group_concat(actor_id, ',') from events.event where kind = 'report_written' order by rowid",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(event_actors, format!("{LOCAL_ACTOR_ID},{LOCAL_ACTOR_ID}"));
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn public_objections_keep_unicode_and_stale_report_anchors_through_review() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-objection-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Objection anchors".to_string(),
                    projection: Projection::Belief,
                },
            )
            .unwrap();
        let report_text = "# Report\n\nMonitoring 😀 cafe\u{301} claim remains contested.";
        let first = store
            .write_synthesis_report(None, &space.id, SynthesisMode::Full, None, report_text, &[])
            .unwrap();
        let quote = "😀 cafe\u{301}";
        let byte_start = report_text.find(quote).unwrap();
        let start = report_text[..byte_start].chars().count();
        let end = start + quote.chars().count();
        let accepted = store
            .submit_public_objection(
                &space.id,
                PublicObjectionInput {
                    report_artifact_id: first.artifact_id.clone(),
                    report_version: first.version,
                    start,
                    end,
                    quote: quote.to_string(),
                    objection: "This wording flattens the governance risk.".to_string(),
                    display_name: Some("  A participant  ".to_string()),
                },
            )
            .unwrap();
        assert_eq!(accepted.kind, ReviewKind::Objection);
        assert_eq!(
            accepted.unverified_display_name.as_deref(),
            Some("A participant")
        );

        let bad_anchor = store.submit_public_objection(
            &space.id,
            PublicObjectionInput {
                report_artifact_id: first.artifact_id.clone(),
                report_version: first.version,
                start: start + 1,
                end,
                quote: quote.to_string(),
                objection: "An invalid scalar range must be rejected.".to_string(),
                display_name: None,
            },
        );
        assert!(bad_anchor.is_err());

        store
            .write_synthesis_report(
                None,
                &space.id,
                SynthesisMode::Selective,
                None,
                "# Report\n\nThe report moved before review.",
                &[],
            )
            .unwrap();
        store
            .accept_review_item(LOCAL_ACTOR_ID, &accepted.id)
            .expect("a stale but valid original anchor remains reviewable");
        let mut causes = store.build_synthesis_context(&space.id).unwrap().causes;
        assert_eq!(causes.len(), 1);
        assert_eq!(causes[0].kind, "objection_submitted");
        assert_eq!(
            causes[0].objection_anchor.as_ref().unwrap().report_version,
            1
        );
        assert_eq!(causes[0].objection_anchor.as_ref().unwrap().quote, quote);
        causes[0].change_classification = Some(ChangeClassification::Weakened);
        causes[0].classification_reason =
            Some("The objection qualifies the monitoring claim.".to_string());
        store
            .write_synthesis_report(
                None,
                &space.id,
                SynthesisMode::Selective,
                Some("Correct the stale rendering"),
                "# Report\n\nThe monitoring claim is qualified by governance risk.",
                &causes,
            )
            .unwrap();
        let history = store.read_report_history(&space.id).unwrap();
        assert_eq!(history.len(), 3);
        let history_cause = &history[2].caused_by[0];
        assert_eq!(
            history_cause
                .objection_anchor
                .as_ref()
                .unwrap()
                .report_version,
            1
        );
        assert_eq!(
            history_cause.objection_anchor.as_ref().unwrap().quote,
            quote
        );
        assert_eq!(
            history_cause.unverified_display_name.as_deref(),
            Some("A participant")
        );
        assert_eq!(
            history_cause.change_classification,
            Some(ChangeClassification::Weakened)
        );

        let current = history.last().unwrap();
        let rejected = store
            .submit_public_objection(
                &space.id,
                PublicObjectionInput {
                    report_artifact_id: current.report.artifact_id.clone(),
                    report_version: current.report.version,
                    start: 0,
                    end: 1,
                    quote: "#".to_string(),
                    objection: "This rejection must not become a cause.".to_string(),
                    display_name: None,
                },
            )
            .unwrap();
        store
            .reject_review_item(LOCAL_ACTOR_ID, &rejected.id)
            .unwrap();
        assert!(
            store
                .build_synthesis_context(&space.id)
                .unwrap()
                .causes
                .is_empty()
        );
        assert_eq!(store.read_report_history(&space.id).unwrap().len(), 3);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn context_projections_are_bounded_and_preserve_missing_semantics() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-context-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Context test".to_string(),
                    projection: Projection::Belief,
                },
            )
            .unwrap();
        let proposal = store
            .propose_entity(
                LOCAL_ACTOR_ID,
                &space.id,
                CreateEntityInput {
                    role: EntityRole::Issue,
                    text: "A context anchor".to_string(),
                },
            )
            .unwrap();
        let doc = store
            .accept_review_item(LOCAL_ACTOR_ID, &proposal.id)
            .unwrap();
        let entity_id = doc
            .nodes
            .iter()
            .find_map(|node| match node {
                DocNode::Entity(entity) => Some(entity.id.clone()),
                _ => None,
            })
            .unwrap();

        let session = store
            .open_session(
                LOCAL_ACTOR_ID,
                OpenSessionInput {
                    anchor_block_id: entity_id.clone(),
                    quote: Some("context".to_string()),
                    stance: Some(Stance::Dispute),
                },
            )
            .unwrap();
        let mut through_message_id = String::new();
        for index in 0..14 {
            let updated = store
                .append_user_message(
                    LOCAL_ACTOR_ID,
                    &session.id,
                    AppendMessageInput {
                        text: format!("turn {index}"),
                    },
                )
                .unwrap();
            through_message_id = updated.turns.last().unwrap().id.clone();
        }
        store
            .append_agent_message(&session.id, "reply to an earlier queued turn")
            .unwrap();
        let reply_context = store
            .build_agent_reply_context(&session.id, &through_message_id, 12)
            .unwrap();
        assert_eq!(reply_context.stance, Some(Stance::Dispute));
        assert_eq!(reply_context.prior_messages.len(), 12);
        assert_eq!(reply_context.prior_messages.first().unwrap().text, "turn 3");
        assert_eq!(
            reply_context.prior_messages.last().unwrap().text,
            "reply to an earlier queued turn"
        );
        let connection = store.open_workspace_connection().unwrap();
        let queued_replies: i64 = connection
            .query_row(
                "select count(*) from operations.worker_job where kind = 'agent_reply' and target_id = ?1 and state = 'queued'",
                params![session.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(queued_replies, 1, "rapid user turns should coalesce");
        drop(connection);

        let target = store
            .submit_input(
                LOCAL_ACTOR_ID,
                &space.id,
                SubmitInput {
                    kind: InputKind::Document,
                    content: "before before TARGET QUOTE after after".to_string(),
                    anchor: None,
                },
            )
            .unwrap();
        let comment = store
            .submit_input(
                LOCAL_ACTOR_ID,
                &space.id,
                SubmitInput {
                    kind: InputKind::Comment,
                    content: "This passage is too strong.".to_string(),
                    anchor: Some(InputAnchor {
                        artifact_id: target.artifact_id,
                        quote: "TARGET QUOTE".to_string(),
                        start: 14,
                        end: 26,
                    }),
                },
            )
            .unwrap();
        let ingest_context = store.build_ingest_context(&comment.artifact_id, 7).unwrap();
        let excerpt = ingest_context.target_excerpt.unwrap();
        assert!(excerpt.contains("TARGET QUOTE"));
        assert!(excerpt.chars().count() <= "TARGET QUOTE".chars().count() + 14);

        store
            .change_entity_state(
                LOCAL_ACTOR_ID,
                &entity_id,
                ChangeEntityStateInput {
                    state: "resolved".to_string(),
                },
            )
            .unwrap();
        let synthesis = store.build_synthesis_context(&space.id).unwrap();
        let state_change = synthesis
            .causes
            .iter()
            .find(|cause| cause.kind == "entity_state_changed")
            .and_then(|cause| cause.state_change.as_ref())
            .unwrap();
        assert_eq!(state_change.old_state, "open");
        assert_eq!(state_change.new_state, "resolved");

        std::fs::remove_dir_all(root).expect("test data should be removed");
    }

    #[test]
    fn imported_thread_context_is_keyed_by_post_artifact() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-thread-context-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).unwrap();
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Two imports".to_string(),
                    projection: Projection::Belief,
                },
            )
            .unwrap();
        let author = LessWrongAuthorInput {
            external_id: "author".to_string(),
            display_name: "Author".to_string(),
            slug: None,
        };
        let make_thread = |post_id: &str, content: &str| LessWrongThreadInput {
            post_id: post_id.to_string(),
            title: format!("Thread {post_id}"),
            slug: post_id.to_string(),
            url: format!("https://www.lesswrong.com/posts/{post_id}/thread"),
            author: author.clone(),
            content: content.to_string(),
            posted_at: "2026-01-01T00:00:00Z".to_string(),
            comments: vec![],
        };
        let first = store
            .import_lesswrong_thread(&space.id, make_thread("first", "FIRST ONLY"))
            .unwrap();
        let second = store
            .import_lesswrong_thread(&space.id, make_thread("second", "SECOND ONLY"))
            .unwrap();
        let first_context = store
            .build_thread_ingest_context(
                &first.artifact_id,
                &space.id,
                std::slice::from_ref(&first.artifact_id),
                200,
                201,
            )
            .unwrap();
        assert_eq!(first_context.new_sources.len(), 1);
        assert_eq!(first_context.new_sources[0].content, "FIRST ONLY");
        assert!(
            !first_context
                .new_sources
                .iter()
                .any(|source| source.artifact_id == second.artifact_id)
        );
        assert_eq!(store.read_imported_sources(&space.id).unwrap().len(), 2);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lesswrong_thread_import_is_idempotent_and_accepts_a_coarse_landscape() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-lesswrong-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Imported debate".to_string(),
                    projection: Projection::Belief,
                },
            )
            .expect("space should be created");
        let author = LessWrongAuthorInput {
            external_id: "authorA".to_string(),
            display_name: "Author A".to_string(),
            slug: Some("author-a".to_string()),
        };
        let commenter = LessWrongAuthorInput {
            external_id: "authorB".to_string(),
            display_name: "Author B".to_string(),
            slug: Some("author-b".to_string()),
        };
        let thread = LessWrongThreadInput {
            post_id: "post123".to_string(),
            title: "A contested claim".to_string(),
            slug: "a-contested-claim".to_string(),
            url: "https://www.lesswrong.com/posts/post123/a-contested-claim".to_string(),
            author: author.clone(),
            content: "The post endorses position A.".to_string(),
            posted_at: "2026-01-01T00:00:00Z".to_string(),
            comments: vec![LessWrongMessageInput {
                external_id: "comment123".to_string(),
                parent_external_id: None,
                author: commenter,
                content: "The comment disputes A and argues B.".to_string(),
                posted_at: "2026-01-02T00:00:00Z".to_string(),
            }],
        };
        let fetch = store
            .enqueue_lesswrong_thread_fetch(LOCAL_ACTOR_ID, &space.id, "post123")
            .expect("fetch should enqueue");
        let duplicate_fetch = store
            .enqueue_lesswrong_thread_fetch(LOCAL_ACTOR_ID, &space.id, "post123")
            .expect("duplicate fetch should reuse the job");
        assert_eq!(fetch, duplicate_fetch);
        store.mark_worker_job_succeeded(&fetch.job_id).unwrap();
        let refresh_fetch = store
            .enqueue_lesswrong_thread_fetch(LOCAL_ACTOR_ID, &space.id, "post123")
            .expect("completed fetch permits refresh");
        assert_ne!(refresh_fetch.job_id, fetch.job_id);

        let first = store
            .import_lesswrong_thread(&space.id, thread.clone())
            .expect("thread imports");
        let second = store
            .import_lesswrong_thread(&space.id, thread)
            .expect("re-import is safe");
        assert_eq!(second.artifact_id, first.artifact_id);
        assert_eq!(second.new_source_count, 0);
        assert!(second.ingest_job_id.is_none());
        store
            .mark_worker_job_failed(
                first.ingest_job_id.as_deref().unwrap(),
                "transient model failure",
            )
            .unwrap();
        let retry = store
            .import_lesswrong_thread(
                &space.id,
                LessWrongThreadInput {
                    post_id: "post123".to_string(),
                    title: "A contested claim".to_string(),
                    slug: "a-contested-claim".to_string(),
                    url: "https://www.lesswrong.com/posts/post123/a-contested-claim".to_string(),
                    author: LessWrongAuthorInput {
                        external_id: "authorA".to_string(),
                        display_name: "Author A".to_string(),
                        slug: None,
                    },
                    content: "The post endorses position A.".to_string(),
                    posted_at: "2026-01-01T00:00:00Z".to_string(),
                    comments: vec![LessWrongMessageInput {
                        external_id: "comment123".to_string(),
                        parent_external_id: None,
                        author: LessWrongAuthorInput {
                            external_id: "authorB".to_string(),
                            display_name: "Author B".to_string(),
                            slug: None,
                        },
                        content: "The comment disputes A and argues B.".to_string(),
                        posted_at: "2026-01-02T00:00:00Z".to_string(),
                    }],
                },
            )
            .expect("failed extraction is retryable through refresh");
        assert_eq!(retry.new_source_count, 2);
        assert_ne!(retry.ingest_job_id, first.ingest_job_id);
        let connection = store.open_workspace_connection().expect("database opens");
        let counts: (i64, i64) = connection.query_row(
            "select (select count(*) from actor where id like 'lw-actor-%'),
                    (select count(*) from artifact where json_extract(metadata_json, '$.inputKind') = 'lesswrongThread')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).expect("counts load");
        assert_eq!(counts, (2, 2));
        drop(connection);
        let second_space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "A second synthesis".to_string(),
                    projection: Projection::Belief,
                },
            )
            .expect("second space should be created");
        let shared = store
            .import_lesswrong_thread(
                &second_space.id,
                LessWrongThreadInput {
                    post_id: "post123".to_string(),
                    title: "A contested claim".to_string(),
                    slug: "a-contested-claim".to_string(),
                    url: "https://www.lesswrong.com/posts/post123/a-contested-claim".to_string(),
                    author,
                    content: "The post endorses position A.".to_string(),
                    posted_at: "2026-01-01T00:00:00Z".to_string(),
                    comments: vec![],
                },
            )
            .expect("the same source can feed another space");
        assert_eq!(shared.artifact_id, first.artifact_id);
        assert_ne!(shared.ingest_job_id, first.ingest_job_id);
        assert_eq!(
            store.read_imported_sources(&second_space.id).unwrap().len(),
            2
        );

        let merge_context = store
            .build_thread_ingest_context(
                &first.artifact_id,
                &space.id,
                &[
                    "lw-post-post123".to_string(),
                    "lw-comment-comment123".to_string(),
                ],
                200,
                201,
            )
            .unwrap();
        let review = store
            .record_thread_ingest_proposal(
                merge_context,
                vec![
                    ThreadPositionProposal {
                        action: "new".to_string(),
                        target_entity_id: None,
                        key: "position-a".to_string(),
                        reasoning: "The post clearly advances this position.".to_string(),
                        text: "Position A should be adopted.".to_string(),
                        source_artifact_ids: vec!["lw-post-post123".to_string()],
                        stances: vec![ThreadStanceProposal {
                            actor_id: "lw-actor-authorA".to_string(),
                            value: "endorse".to_string(),
                            evidence_artifact_id: "lw-post-post123".to_string(),
                            stated_confidence: None,
                            falsifiers: vec![],
                        }],
                    },
                    ThreadPositionProposal {
                        action: "new".to_string(),
                        target_entity_id: None,
                        key: "position-b".to_string(),
                        reasoning: "The comment advances a distinct position.".to_string(),
                        text: "Position B better explains the evidence.".to_string(),
                        source_artifact_ids: vec!["lw-comment-comment123".to_string()],
                        stances: vec![ThreadStanceProposal {
                            actor_id: "lw-actor-authorB".to_string(),
                            value: "endorse".to_string(),
                            evidence_artifact_id: "lw-comment-comment123".to_string(),
                            stated_confidence: None,
                            falsifiers: vec![],
                        }],
                    },
                ],
            )
            .expect("landscape reaches review")
            .expect("non-empty delta creates review");
        let doc = store
            .accept_review_item(LOCAL_ACTOR_ID, &review.id)
            .expect("landscape accepts");
        assert_eq!(
            doc.nodes
                .iter()
                .filter(|node| matches!(node, DocNode::Entity(_)))
                .count(),
            2
        );
        let synthesis_context = store.build_synthesis_context(&space.id).unwrap();
        assert!(synthesis_context.sources.iter().all(|source| {
            source
                .author_name
                .as_deref()
                .is_some_and(|name| !name.is_empty())
                && source
                    .source_url
                    .as_deref()
                    .is_some_and(|url| url.starts_with("https://"))
        }));
        assert!(
            doc.nodes
                .iter()
                .filter_map(|node| match node {
                    DocNode::Entity(entity) => Some(&entity.holders),
                    _ => None,
                })
                .all(|holders| holders.len() == 1 && holders[0].evidence.len() == 1)
        );
        let position_a_id = doc
            .nodes
            .iter()
            .find_map(|node| match node {
                DocNode::Entity(entity) if entity.text == "Position A should be adopted." => {
                    Some(entity.id.clone())
                }
                _ => None,
            })
            .unwrap();
        let refreshed = store
            .import_lesswrong_thread(
                &space.id,
                LessWrongThreadInput {
                    post_id: "post123".to_string(),
                    title: "A contested claim".to_string(),
                    slug: "a-contested-claim".to_string(),
                    url: "https://www.lesswrong.com/posts/post123/a-contested-claim".to_string(),
                    author: LessWrongAuthorInput {
                        external_id: "authorA".to_string(),
                        display_name: "Author A".to_string(),
                        slug: None,
                    },
                    content: "The post endorses position A.".to_string(),
                    posted_at: "2026-01-01T00:00:00Z".to_string(),
                    comments: vec![
                        LessWrongMessageInput {
                            external_id: "comment123".to_string(),
                            parent_external_id: None,
                            author: LessWrongAuthorInput {
                                external_id: "authorB".to_string(),
                                display_name: "Author B".to_string(),
                                slug: None,
                            },
                            content: "The comment disputes A and argues B.".to_string(),
                            posted_at: "2026-01-02T00:00:00Z".to_string(),
                        },
                        LessWrongMessageInput {
                            external_id: "comment456".to_string(),
                            parent_external_id: Some("comment123".to_string()),
                            author: LessWrongAuthorInput {
                                external_id: "authorA".to_string(),
                                display_name: "Author A".to_string(),
                                slug: None,
                            },
                            content: "I hold position A with high confidence after new evidence."
                                .to_string(),
                            posted_at: "2026-01-03T00:00:00Z".to_string(),
                        },
                    ],
                },
            )
            .unwrap();
        assert_eq!(refreshed.new_source_count, 1);
        let refresh_context = store
            .build_thread_ingest_context(
                &first.artifact_id,
                &space.id,
                &["lw-comment-comment456".to_string()],
                200,
                201,
            )
            .unwrap();
        assert_eq!(refresh_context.standing_positions.len(), 2);
        let attachment = store
            .record_thread_ingest_proposal(
                refresh_context,
                vec![ThreadPositionProposal {
                    action: "attach".to_string(),
                    target_entity_id: Some(position_a_id.clone()),
                    key: "position-a-evidence".to_string(),
                    text: "I hold position A with high confidence after new evidence.".to_string(),
                    reasoning: "This is the same position with new evidence.".to_string(),
                    source_artifact_ids: vec!["lw-comment-comment456".to_string()],
                    stances: vec![ThreadStanceProposal {
                        actor_id: "lw-actor-authorA".to_string(),
                        value: "endorse".to_string(),
                        evidence_artifact_id: "lw-comment-comment456".to_string(),
                        stated_confidence: Some("high confidence".to_string()),
                        falsifiers: vec![],
                    }],
                }],
            )
            .unwrap()
            .unwrap();
        let refreshed_doc = store
            .accept_review_item(LOCAL_ACTOR_ID, &attachment.id)
            .unwrap();
        assert_eq!(
            refreshed_doc
                .nodes
                .iter()
                .filter(|node| matches!(node, DocNode::Entity(_)))
                .count(),
            2
        );
        let atlas_holder = refreshed_doc
            .nodes
            .iter()
            .find_map(|node| match node {
                DocNode::Entity(entity) if entity.id == position_a_id => entity.holders.first(),
                _ => None,
            })
            .expect("position A keeps its holder");
        assert_eq!(atlas_holder.evidence.len(), 1);
        assert_eq!(
            atlas_holder.evidence[0].artifact_id,
            "lw-comment-comment456"
        );
        assert_eq!(
            atlas_holder.evidence[0].authored_at.as_deref(),
            Some("2026-01-03T00:00:00Z")
        );
        assert_eq!(
            atlas_holder.stated_confidence.as_deref(),
            Some("high confidence")
        );
        assert_eq!(atlas_holder.history.len(), 1);
        assert_eq!(
            atlas_holder.history[0].evidence[0].artifact_id,
            "lw-post-post123"
        );
        assert_eq!(
            atlas_holder.history[0].evidence[0].authored_at.as_deref(),
            Some("2026-01-01T00:00:00Z")
        );
        assert_eq!(atlas_holder.history[0].stated_confidence, None);
        let connection = store.open_workspace_connection().expect("database opens");
        assert_eq!(
            connection
                .query_row(
                    "select count(*) from relation where kind = 'derived_from'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row(
                    "select count(*) from stance where actor_id like 'lw-actor-%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            3
        );
        assert_eq!(
            connection.query_row(
                "select count(*) from events.event where kind = 'lesswrong_landscape_delta_accepted' and target_id = ?1",
                params![position_a_id], |row| row.get::<_, i64>(0),
            ).unwrap(),
            2
        );
        drop(connection);
        std::fs::remove_dir_all(root).expect("test data should be removed");
    }

    #[test]
    fn large_lesswrong_imports_are_chunked_without_losing_new_sources() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-lesswrong-chunks-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).unwrap();
        let space = store
            .create_space(
                LOCAL_ACTOR_ID,
                CreateSpaceInput {
                    title: "Large debate".to_string(),
                    projection: Projection::Belief,
                },
            )
            .unwrap();
        let author = LessWrongAuthorInput {
            external_id: "author".to_string(),
            display_name: "Author".to_string(),
            slug: None,
        };
        let comments = (0..THREAD_MERGE_BATCH_SIZE + 1)
            .map(|index| LessWrongMessageInput {
                external_id: format!("comment-{index}"),
                parent_external_id: None,
                author: author.clone(),
                content: format!("Comment {index}"),
                posted_at: format!("2026-01-01T00:{:02}:00Z", index % 60),
            })
            .collect();
        let receipt = store
            .import_lesswrong_thread(
                &space.id,
                LessWrongThreadInput {
                    post_id: "large-post".to_string(),
                    title: "Large".to_string(),
                    slug: "large".to_string(),
                    url: "https://www.lesswrong.com/posts/large-post/large".to_string(),
                    author,
                    content: "Root".to_string(),
                    posted_at: "2026-01-01T00:00:00Z".to_string(),
                    comments,
                },
            )
            .unwrap();
        assert_eq!(receipt.new_source_count, THREAD_MERGE_BATCH_SIZE + 2);
        let connection = store.open_workspace_connection().unwrap();
        let (jobs, sources): (i64, i64) = connection
            .query_row(
                "select count(*), sum(json_array_length(payload_json, '$.newSourceArtifactIds'))
             from operations.worker_job where kind = 'lesswrong_thread_ingest'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(jobs, 2);
        assert_eq!(sources as usize, THREAD_MERGE_BATCH_SIZE + 2);
        drop(connection);
        std::fs::remove_dir_all(root).unwrap();
    }
}
