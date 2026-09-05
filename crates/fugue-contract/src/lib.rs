use serde::{Deserialize, Serialize};
use specta::{
    Type,
    ts::{BigIntExportBehavior, ExportConfiguration, TsExportError},
};

pub use fugue_store::{
    AllowanceGrantResult, AppendMessageInput, ArmExperimentInjectionInput, ArtifactReadModel,
    AuthActor, BankSnapshot, BirthSocietyAgentInput, BoardFeed, BoardPost, BoardPostBody,
    BoardPostKind, BoardReadReceipt, BoardSpace, ChangeClassification, ChangeEntityStateInput,
    ContractKind, CostSummary, CostTotals, CreateBoardPostInput, CreateBoardSpaceInput,
    CreateEntityInput, CreateEscrowInput, CreateSocietyAgentInput, CreateSpaceInput,
    DeclareConditionInput, DiffKind, DiffLine, DirectoryEntry, DirectoryRegistrationInput, DocNode,
    EditEntityTextInput, EmitExperimentSignalInput, EntityNode, EntityRole, EscrowReadModel,
    EscrowResolutionAuthority, ExperimentInjectedMessage, ExperimentInjection,
    ExperimentInjectionRecipient, ExperimentInjectionRecipients, ExperimentInjectionState,
    ExperimentInjectionTrigger, ExternalMarket, FalsifierGroup, FalsifierReadModel, FeedEntry,
    FeedEntryKind, HistoricalStance, HolderCondition, ImportedSource, InputAnchor, InputKind,
    InputReceipt, MintObolsInput, Neighbor, NewspaperEntry, NewspaperSections, ObjectionAnchor,
    ObjectionOutcome, ObolAccountBalance, ObolLedgerEntry, OpenSessionInput, PositionHolder,
    Projection, PublicObjectionInput, PutRefreshScheduleInput, RecordSocietyTranscriptInput,
    RecordSocietyUsageInput, RefreshSchedule, ReportCause, ReportDiffHunk, ReportProvenanceAnchor,
    ReportVersion, ReportVersionDiff, ReviewItem, ReviewKind, ReviewSessionProvenance,
    SessionAnchor, SessionReadModel, SessionTurn, SleepStartedInput, SocietyAdminAgentDetail,
    SocietyAdminAgentSummary, SocietyAdminAuthoredPost, SocietyAdminBankSummary,
    SocietyAdminLedgerEntry, SocietyAdminOverview, SocietyAdminTranscript,
    SocietyAdminTranscriptEntry, SocietyAdminTranscriptRole, SocietyAdminWakeReceipt,
    SocietyAgentCredential, SocietyAgentLifecycle, SocietyAgentPhase, SocietyBudget,
    SocietyComputeBacking, SocietyIdentity, SocietyModelUsageTotal, SocietyNewspaper,
    SocietyTranscript, SocietyUsageRecord, SocietyWakeContext, SocietyWakePaidPost,
    SocietyWakeStart, SourceLink, SpaceCostTotals, SpaceRef, StageCostTotals, Stance, SubmitInput,
    SynthesisDoc, SynthesisMode, SynthesisReport, SynthesizeInput, TransferObolsInput, TurnAuthor,
    WakeDebitReceipt, WakeStartedInput, WorkerJobReceipt,
};

pub fn export_typescript(path: &str) -> Result<(), TsExportError> {
    let config = ExportConfiguration::default().bigint(BigIntExportBehavior::Number);
    specta::export::ts_with_cfg(path, &config)
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct HealthResponse {
    pub status: String,
    pub engine: String,
    pub storage: String,
    pub instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[specta(export)]
pub struct AuthInput {
    pub username: String,
    pub password: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn selective_synthesis_requires_its_instruction_at_deserialization() {
        let selective: SynthesizeInput = serde_json::from_value(json!({
            "mode": "selective",
            "instruction": "Tighten the dissent section"
        }))
        .unwrap();
        assert_eq!(
            selective,
            SynthesizeInput::Selective {
                instruction: "Tighten the dissent section".to_string()
            }
        );
        assert!(serde_json::from_value::<SynthesizeInput>(json!({"mode": "selective"})).is_err());
    }

    #[test]
    fn entity_wire_fields_are_camel_case_and_optional() {
        let node = DocNode::Entity(EntityNode {
            id: "entity-1".to_string(),
            role: EntityRole::Issue,
            state: "open".to_string(),
            text: "A claim".to_string(),
            posterior: None,
            markets: None,
            half_life_days: Some(30),
            dissent: None,
            holders: Vec::new(),
            neighbors: Vec::new(),
        });

        assert_eq!(
            serde_json::to_value(node).unwrap(),
            json!({
                "kind": "entity",
                "id": "entity-1",
                "role": "issue",
                "state": "open",
                "text": "A claim",
                "halfLifeDays": 30,
                "holders": [],
                "neighbors": []
            })
        );
    }

    #[test]
    fn health_identifies_the_configured_society_instance() {
        let health = HealthResponse {
            status: "ready".to_string(),
            engine: "fugue-core".to_string(),
            storage: "ready".to_string(),
            instance_id: "experiment-7".to_string(),
            model: None,
        };

        assert_eq!(
            serde_json::to_value(health).unwrap(),
            json!({
                "status": "ready",
                "engine": "fugue-core",
                "storage": "ready",
                "instanceId": "experiment-7"
            })
        );
    }
}
