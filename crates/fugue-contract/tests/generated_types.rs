const GENERATED: &str = include_str!("../../../frontend/src/runtime/generated.ts");

#[test]
fn generated_contract_covers_wire_shapes_used_by_the_ui() {
    assert!(GENERATED.contains(r#"({ kind: "entity" } & EntityNode)"#));
    assert!(GENERATED.contains(
        r#"export type SynthesizeInput = { mode: "full" } | { mode: "selective"; instruction: string }"#
    ));
    assert!(GENERATED.contains(
        "export type SynthesisDoc = { spaceId: string; title: string; version: number; compiledAt: string; nodes: DocNode[] }"
    ));
    assert!(GENERATED.contains("sourceArtifactId?: string | null"));
    assert!(GENERATED.contains("anchor?: InputAnchor | null"));
    assert!(GENERATED.contains("posterior?: number | null"));
    assert!(GENERATED.contains("markets?: ExternalMarket[] | null"));
    assert!(GENERATED.contains("halfLifeDays?: number | null"));
    assert!(GENERATED.contains("holders: PositionHolder[]"));
    assert!(GENERATED.contains("neighbors: Neighbor[]"));
    assert!(GENERATED.contains("fromSession?: ReviewSessionProvenance | null"));
    assert!(GENERATED.contains("stance?: Stance | null"));
    assert!(GENERATED.contains("export type PublicObjectionInput"));
    assert!(GENERATED.contains("export type ReportVersion"));
    assert!(GENERATED.contains("export type CostSummary"));
    assert!(GENERATED.contains("export type AuthInput"));
    assert!(
        GENERATED
            .contains("export type EntityRole = \"issue\" | \"goal\" | \"action\" | \"condition\"")
    );
    assert!(GENERATED.contains("conditions?: HolderCondition[]"));
    assert!(GENERATED.contains("export type ReportVersionDiff"));
    assert!(GENERATED.contains("export type FeedEntry"));
    assert!(GENERATED.contains("export type RefreshSchedule"));
    assert!(GENERATED.contains("export type PutRefreshScheduleInput"));
    assert!(GENERATED.contains("export type FalsifierGroup"));
    assert!(GENERATED.contains("export type DeclareConditionInput"));
    assert!(GENERATED.contains("export type SocietyIdentity"));
    assert!(GENERATED.contains("export type RecordSocietyUsageInput"));
    assert!(GENERATED.contains("export type SocietyUsageRecord"));
    assert!(GENERATED.contains("export type SocietyAdminOverview"));
    assert!(GENERATED.contains("export type SocietyAdminAgentDetail"));
    assert!(GENERATED.contains("export type SocietyAdminTranscriptEntry"));
}
