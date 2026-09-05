use fugue_core::{ResponseFunctionTool, ResponseJsonSchema, ResponsesRequest};
use fugue_store::{
    ReportCause, SocietyNewspaperContext, StoreInfo, SynthesisContext, THREAD_MERGE_BATCH_SIZE,
    ThreadIngestContext,
};
use serde::Serialize;
use serde_json::{Value, json};

pub const SESSION_HISTORY_TURNS: usize = 12;
pub const COMMENT_TARGET_EXCERPT_RADIUS_CHARS: usize = 1_200;
pub const STANDING_LANDSCAPE_MAX_POSITIONS: usize = 200;
pub const NEW_DISCOURSE_MAX_SOURCES: usize = THREAD_MERGE_BATCH_SIZE;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase", tag = "block")]
pub enum ContextBlock {
    SessionAnchor {
        include_anchor_text: bool,
        include_quote: bool,
    },
    DeclaredStance,
    CompiledDocument {
        shape: DocumentShape,
        bound: Bound,
    },
    ThreadTurns {
        bound: Bound,
    },
    RawInput {
        bound: Bound,
    },
    AnnotatedTarget {
        bound: Bound,
    },
    StandingLandscape {
        bound: Bound,
    },
    NewDiscourse {
        keyed_by: &'static str,
        bound: Bound,
    },
    CurrentReport {
        shape: ReportShape,
        bound: Bound,
    },
    CauseWindow {
        bound: Bound,
    },
    ProvenanceSources {
        authorship: bool,
        source_url: bool,
        bound: Bound,
    },
    RoutingInstruction {
        bound: Bound,
    },
    CurrentNewspaper {
        bound: Bound,
    },
    PublicBoardPosts {
        bound: Bound,
    },
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind", content = "value")]
pub enum Bound {
    Single,
    All,
    Latest,
    Last(usize),
    SincePreviousReport,
    SinceNewspaper,
    RadiusChars(usize),
    MaxItems(usize),
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DocumentShape {
    FlatNodeText,
    Structured,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ReportShape {
    FullReadModel,
    ContentOnly,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum OutputContract {
    FreeText,
    JsonSchema { name: &'static str },
    RequiredTools { names: &'static [&'static str] },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    pub name: &'static str,
    pub instructions: &'static str,
    pub blocks: &'static [ContextBlock],
    pub output: OutputContract,
}

const SESSION_BLOCKS: &[ContextBlock] = &[
    ContextBlock::SessionAnchor {
        include_anchor_text: true,
        include_quote: true,
    },
    ContextBlock::DeclaredStance,
    ContextBlock::CompiledDocument {
        shape: DocumentShape::FlatNodeText,
        bound: Bound::All,
    },
    ContextBlock::ThreadTurns {
        bound: Bound::Last(SESSION_HISTORY_TURNS),
    },
];
const COARSE_INGEST_BLOCKS: &[ContextBlock] = &[
    ContextBlock::RawInput {
        bound: Bound::Single,
    },
    ContextBlock::AnnotatedTarget {
        bound: Bound::RadiusChars(COMMENT_TARGET_EXCERPT_RADIUS_CHARS),
    },
];
const THREAD_INGEST_BLOCKS: &[ContextBlock] = &[
    ContextBlock::StandingLandscape {
        bound: Bound::MaxItems(STANDING_LANDSCAPE_MAX_POSITIONS),
    },
    ContextBlock::NewDiscourse {
        keyed_by: "newSourceArtifactIds",
        bound: Bound::MaxItems(NEW_DISCOURSE_MAX_SOURCES),
    },
];
const CLASSIFIER_BLOCKS: &[ContextBlock] = &[
    ContextBlock::CurrentReport {
        shape: ReportShape::FullReadModel,
        bound: Bound::Latest,
    },
    ContextBlock::CauseWindow {
        bound: Bound::SincePreviousReport,
    },
];
const FULL_GENERATION_BLOCKS: &[ContextBlock] = &[
    ContextBlock::CompiledDocument {
        shape: DocumentShape::Structured,
        bound: Bound::All,
    },
    ContextBlock::ProvenanceSources {
        authorship: true,
        source_url: true,
        bound: Bound::All,
    },
    ContextBlock::CauseWindow {
        bound: Bound::SincePreviousReport,
    },
    ContextBlock::CurrentReport {
        shape: ReportShape::FullReadModel,
        bound: Bound::Latest,
    },
];
const SELECTIVE_EDIT_BLOCKS: &[ContextBlock] = &[
    ContextBlock::RoutingInstruction {
        bound: Bound::Single,
    },
    ContextBlock::CauseWindow {
        bound: Bound::SincePreviousReport,
    },
    ContextBlock::CurrentReport {
        shape: ReportShape::ContentOnly,
        bound: Bound::Latest,
    },
    ContextBlock::CompiledDocument {
        shape: DocumentShape::Structured,
        bound: Bound::All,
    },
];
const SOCIETY_NEWSPAPER_BLOCKS: &[ContextBlock] = &[
    ContextBlock::CurrentNewspaper {
        bound: Bound::Latest,
    },
    ContextBlock::PublicBoardPosts {
        bound: Bound::SinceNewspaper,
    },
];

pub struct AgentRequest {
    pub agent_name: &'static str,
    pub request: ResponsesRequest,
}

pub static AGENT_DEFINITIONS: &[AgentDefinition] = &[
    AgentDefinition {
        name: "session_reply",
        instructions: "Reply as a concise synthesis assistant. Ground the reply in the provided anchor, quote, declared stance, surrounding document, and prior messages. Do not invent sources. Keep it to one or two short sentences.",
        blocks: SESSION_BLOCKS,
        output: OutputContract::FreeText,
    },
    AgentDefinition {
        name: "coarse_ingest",
        instructions: "Propose one coarse entity that lets this input enter the living synthesis. Preserve the input's central contribution as a single issue, goal, or action. Do not atomize it into claims, enumerate subpoints, or invent evidence. A comment may become an issue about its annotated passage; use the target excerpt to interpret it. Return only the requested structured object.",
        blocks: COARSE_INGEST_BLOCKS,
        output: OutputContract::JsonSchema {
            name: "coarse_ingest_proposal",
        },
    },
    AgentDefinition {
        name: "lesswrong_landscape",
        instructions: "Merge only the supplied NEW discourse into the space's standing accepted position landscape. Return 0-6 coarse findings, never a comment summary or one finding per comment. Each finding either attaches new evidence and/or clear holders to one genuinely identical standing position, or proposes a new coarse position. Position identity is an ambassador problem: attach only when the source author would endorse the standing position's wording; when identity is uncertain, default to a new position rather than flattening dissent. For attach, preserve the standing entity ID and explain whether the material strengthens, weakens, or otherwise qualifies it. For new, provide no target ID. Every finding must cite only specific new-source artifact IDs. Add a stance only where that author's own text in one specific cited source clearly endorses or disputes the target position, and set evidenceArtifactId to that exact source. If that same evidencing text directly states confidence or credence about this position, copy the shortest literal qualifier into statedConfidence (for example '~70%', 'weakly hold', or 'high confidence'). Never translate qualitative confidence into a number and never infer confidence; use null when it is absent or ambiguous. For each stance, include 0-3 falsifiers only when that same evidencing text explicitly says what observation or condition would move the author's stance. Copy the complete falsifier statement verbatim into quote and state its testable condition concisely in condition. The quote must be an exact substring of evidenceArtifactId. Suppress a falsifier whenever its wording, author, evidence, or target is ambiguous; never guess. Falsifiers contain no IDs. Prefer the newer postedAt when new text clearly supersedes the same actor's stance. Use only IDs in the input. Empty findings is valid when new discourse changes no position. Return only the requested structured object.",
        blocks: THREAD_INGEST_BLOCKS,
        output: OutputContract::JsonSchema {
            name: "lesswrong_position_landscape",
        },
    },
    AgentDefinition {
        name: "synthesis_classifier",
        instructions: "Apply the synthesis quality contract to every newly accepted input. An objection_submitted input is an unverified correction request anchored to the quoted wording of an original report version: assess the objection's challenge to faithful representation, never treat its optional display name as verified identity, and preserve its anchor as record rather than silently moving it to current wording. Judge redundancy ONLY against the reader-visible currentReport before the input; the accepted input and any compiled entity created from it do not count as already reported. Check redundancy first: if every substantive claim is already reader-visible, classify redundant even when the repeated wording says it strengthens or weakens a side; a restatement is not new evidence. Only after ruling out redundancy, classify each event exactly once by its position-level effect: strengthened means it makes an existing position's own case more compelling; weakened means it challenges a premise, expected effectiveness, evidence, precision, or load-bearing assumption of an existing position without asserting a mutually exclusive rival conclusion; new-tension means it creates a genuine two-sided incompatibility; redundant means every substantive claim is already reader-visible in currentReport. New supporting evidence explicitly offered for an existing named side strengthens that side even when the supporting consideration itself is not yet reader-visible; it is not a new tension merely because the evidence is new. A compatible vulnerability of a position weakens that position—it does not strengthen it. Near-paraphrases are redundant. Also route integration: selective preserves unaffected bytes; full is only for a genuinely reorganized landscape or no current report. The classification and reason must agree and name the affected position; retain distinctive proper names from the input for named evidence or audits so readers can tell exactly what moved the position. For visibleReaction, write the reader-facing sentence itself, never an instruction. A weakened reaction says what became less credible; a new-tension reaction neutrally names both sides and retains explicit side labels verbatim. Return every supplied eventId unchanged and no others.",
        blocks: CLASSIFIER_BLOCKS,
        output: OutputContract::JsonSchema {
            name: "synthesis_quality_assessment",
        },
    },
    AgentDefinition {
        name: "full_generation",
        instructions: "Write the complete publishable State-of-X report under a strict quality contract. Accepted public objections are correction requests, not automatically true source claims: correct the challenged rendering when supported by the supplied record, keep uncertainty explicit, and never present an optional unverified display name as authenticated attribution. Use exactly these reader-visible top-level sections after the title: ## Source record, ## Synthesis interpretation, and ## Cruxes. Source record states only source-supported claims. For every harvested position with holder/evidence data, include its accepted position text verbatim once in Source record, preceded by a bold holder label in the form **Name:**; this exact wording lets the server attach only relation-backed provenance without guessing. Attribute a person only when holder/evidence data supports that actor-position link. Treat prior holder stances only as an inspectable record: do not narrate a person's movement, infer why they changed, or claim that evidence moved them. Native accepted claims are materialized separately from their accepted coarse entity text with authenticated attribution; use them to update interpretation and cruxes without duplicating their Source record bullet. Phrase each harvested compressed attribution so that its author could endorse it. Synthesis interpretation contains only clearly signposted inferences, never disguised record. Cruxes states each genuine conflict as one neutral, signable sentence naming what both sides share and exactly what they disagree about; do not average or resolve them. Integrate each classified accepted input visibly: strengthened adds support/content, weakened identifies what it undercuts, new-tension creates or sharpens a crux referencing both sides and retains any explicitly named side labels verbatim, redundant adds nothing. Falsifiers and conditional stances are rendered by deterministic read models; do not narrate conditions or falsifiers in report prose. Do not invent sources, claims, or holders. Do not mention JSON, workers, or instructions. Return clean plain text.",
        blocks: FULL_GENERATION_BLOCKS,
        output: OutputContract::FreeText,
    },
    AgentDefinition {
        name: "selective_edit",
        instructions: "Integrate every classified accepted input into the current report using only the minimal text-edit tools and preserve every unaffected byte. For an accepted public objection, correct or qualify the specifically quoted rendering without treating the objection or its unverified display name as source proof. Keep the required Source record, Synthesis interpretation, and Cruxes sections distinct. Treat prior holder stances only as an inspectable record: do not narrate a person's movement, infer why they changed, or claim that evidence moved them. Put each claim near the report material it semantically engages, not merely under the nearest convenient heading. Native accepted claims are materialized separately with authenticated attribution, so do not duplicate their Source record bullet. Put their effect and all other inferences only in Synthesis interpretation. For new-tension or weakening, edit Cruxes to state what conflict or position-level support changed without smoothing dissent. For strengthened, add the supported consideration in the appropriate section and retain distinctive named terms. Redundant inputs require no content, but this path is used only when at least one input is substantive. Falsifiers and conditional stances belong only in deterministic UI; never add conditions or falsifiers to report prose. For insert_after_anchor, anchor a complete markdown block or heading and supply a complete markdown block; the server enforces blank-line boundaries. Never rewrite the report or describe the edit.",
        blocks: SELECTIVE_EDIT_BLOCKS,
        output: OutputContract::RequiredTools {
            names: &["replace_exact", "insert_after_anchor"],
        },
    },
    AgentDefinition {
        name: "society_newspaper",
        instructions: "Update the complete compact Society Newspaper from its current structured sections and the new public Board posts. Return only the requested structured object. Keep three distinct layers: sourceRecord contains source-supported claims, synthesisInterpretation contains clearly marked synthesis, and openQuestions contains unresolved questions or disagreements. Preserve disagreement instead of averaging it. Attribute claims faithfully. Make each text one trimmed paragraph without line breaks. Each entry must cite one or more supplied P references. Use no reference outside the supplied context. Represent every requiredNewReference in at least one entry. Keep useful prior entries unless new public material supersedes them. Do not mention private messages, workers, JSON, or these instructions.",
        blocks: SOCIETY_NEWSPAPER_BLOCKS,
        output: OutputContract::JsonSchema {
            name: "society_newspaper_sections",
        },
    },
];

pub fn agent_names() -> impl Iterator<Item = &'static str> {
    AGENT_DEFINITIONS.iter().map(|agent| agent.name)
}

pub fn with_validation_feedback(
    mut request: ResponsesRequest,
    validation_error: &str,
) -> anyhow::Result<ResponsesRequest> {
    let mut input: Value = serde_json::from_str(&request.input)?;
    let object = input
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("agent context input must be a JSON object"))?;
    object.insert(
        "validationFeedback".to_string(),
        json!({
            "error": validation_error,
            "instruction": "Correct the prior response so it satisfies the declared output and domain contract. Return the complete corrected result only."
        }),
    );
    request.input = serde_json::to_string(&input)?;
    Ok(request)
}

fn definition(name: &str) -> &'static AgentDefinition {
    AGENT_DEFINITIONS
        .iter()
        .find(|agent| agent.name == name)
        .expect("declared agent")
}

fn request(agent_name: &'static str, input: Value) -> anyhow::Result<AgentRequest> {
    let agent = definition(agent_name);
    let (json_schema, tools, require_tool_call) = match agent.output {
        OutputContract::FreeText => (None, vec![], false),
        OutputContract::JsonSchema { name } => (Some(json_schema(name)?), vec![], false),
        OutputContract::RequiredTools { names } => {
            let tools = text_edit_tools();
            let actual_names = tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>();
            anyhow::ensure!(
                actual_names == names,
                "declared tool contract does not match tool definitions"
            );
            (None, tools, true)
        }
    };
    let keys = input
        .as_object()
        .map(|object| object.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    eprintln!(
        "context_composed agent={} keys={} blocks={}",
        agent.name,
        keys.join(","),
        serde_json::to_string(agent.blocks)?,
    );
    Ok(AgentRequest {
        agent_name,
        request: ResponsesRequest {
            instructions: agent.instructions.to_string(),
            input: serde_json::to_string(&input)?,
            json_schema,
            tools,
            require_tool_call,
        },
    })
}

pub fn session_reply(
    store: &StoreInfo,
    thread_id: &str,
    through_message_id: &str,
) -> anyhow::Result<AgentRequest> {
    let context =
        store.build_agent_reply_context(thread_id, through_message_id, SESSION_HISTORY_TURNS)?;
    request("session_reply", serde_json::to_value(context)?)
}

pub fn coarse_ingest(store: &StoreInfo, artifact_id: &str) -> anyhow::Result<AgentRequest> {
    let context = store.build_ingest_context(artifact_id, COMMENT_TARGET_EXCERPT_RADIUS_CHARS)?;
    request("coarse_ingest", serde_json::to_value(context)?)
}

pub fn lesswrong_landscape(
    store: &StoreInfo,
    artifact_id: &str,
    space_id: &str,
    new_source_artifact_ids: &[String],
) -> anyhow::Result<(AgentRequest, ThreadIngestContext)> {
    let context = store.build_thread_ingest_context(
        artifact_id,
        space_id,
        new_source_artifact_ids,
        STANDING_LANDSCAPE_MAX_POSITIONS,
        NEW_DISCOURSE_MAX_SOURCES,
    )?;
    let request = request("lesswrong_landscape", serde_json::to_value(&context)?)?;
    Ok((request, context))
}

pub fn classifier(context: &SynthesisContext) -> anyhow::Result<AgentRequest> {
    request(
        "synthesis_classifier",
        json!({
            "currentReport": current_report(context),
            "newAcceptedInputs": cause_window(&context.causes),
        }),
    )
}

pub fn full_generation(context: &SynthesisContext) -> anyhow::Result<AgentRequest> {
    request(
        "full_generation",
        json!({
            "spaceId": context.space_id,
            "title": context.title,
            "compiledDocument": compiled_document(context),
            "sources": provenance_sources(context),
            "newAcceptedInputs": cause_window(&context.causes),
            "currentReport": current_report(context),
        }),
    )
}

pub fn selective_edit(
    context: &SynthesisContext,
    routing_reason: &str,
) -> anyhow::Result<AgentRequest> {
    let current_report = context
        .current_report
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("selective synthesis requires a current report"))?;
    request(
        "selective_edit",
        json!({
            "routingReason": routing_reason,
            "newAcceptedInputs": cause_window(&context.causes),
            "currentReport": current_report.content,
            "compiledState": compiled_document(context),
        }),
    )
}

pub fn society_newspaper(context: &SocietyNewspaperContext) -> anyhow::Result<AgentRequest> {
    request(
        "society_newspaper",
        json!({
            "space": {
                "id": context.space_id,
                "name": context.space_name,
            },
            "current": context.current_sections.as_ref().map(|sections| json!({
                "version": context.current_version,
                "sections": sections,
            })),
            "newPublicPosts": context.new_public_posts,
            "requiredNewReferences": context
                .new_public_posts
                .iter()
                .map(|post| post.reference.as_str())
                .collect::<Vec<_>>(),
        }),
    )
}

fn current_report(context: &SynthesisContext) -> Value {
    serde_json::to_value(&context.current_report).expect("serializable report")
}

fn compiled_document(context: &SynthesisContext) -> Value {
    serde_json::to_value(&context.compiled_document).expect("serializable compiled document")
}

fn provenance_sources(context: &SynthesisContext) -> Value {
    serde_json::to_value(&context.sources).expect("serializable provenance sources")
}

fn cause_window(causes: &[ReportCause]) -> Vec<Value> {
    causes
        .iter()
        .map(|cause| {
            json!({
                "eventId": cause.event_id,
                "kind": cause.kind,
                "label": cause.label,
                "source": cause.source,
                "authorName": cause.author_name,
                "unverifiedDisplayName": cause.unverified_display_name,
                "objectionAnchor": cause.objection_anchor,
                "inputText": cause.input_text,
                "stateChange": cause.state_change,
                "changeClassification": cause.change_classification,
                "classificationReason": cause.classification_reason,
            })
        })
        .collect()
}

fn json_schema(name: &str) -> anyhow::Result<ResponseJsonSchema> {
    let schema = match name {
        "coarse_ingest_proposal" => json!({
            "type": "object",
            "properties": {
                "role": { "type": "string", "enum": ["issue", "goal", "action"] },
                "text": { "type": "string", "minLength": 1 }
            },
            "required": ["role", "text"],
            "additionalProperties": false
        }),
        "lesswrong_position_landscape" => json!({
            "type": "object",
            "properties": {
                "positions": {
                    "type": "array", "minItems": 0, "maxItems": 6,
                    "items": {
                        "type": "object",
                        "properties": {
                            "action": { "type": "string", "enum": ["attach", "new"] },
                            "targetEntityId": { "type": ["string", "null"] },
                            "key": { "type": "string", "minLength": 1 },
                            "text": { "type": "string", "minLength": 1 },
                            "reasoning": { "type": "string", "minLength": 1 },
                            "sourceArtifactIds": { "type": "array", "minItems": 1, "items": { "type": "string" } },
                            "stances": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "actorId": { "type": "string" },
                                        "value": { "type": "string", "enum": ["endorse", "dispute"] },
                                        "evidenceArtifactId": { "type": "string", "minLength": 1 },
                                        "statedConfidence": { "type": ["string", "null"] },
                                        "falsifiers": {
                                            "type": "array", "minItems": 0, "maxItems": 3,
                                            "items": {
                                                "type": "object",
                                                "properties": {
                                                    "quote": { "type": "string", "minLength": 1 },
                                                    "condition": { "type": "string", "minLength": 1 }
                                                },
                                                "required": ["quote", "condition"],
                                                "additionalProperties": false
                                            }
                                        }
                                    },
                                    "required": ["actorId", "value", "evidenceArtifactId", "statedConfidence", "falsifiers"],
                                    "additionalProperties": false
                                }
                            }
                        },
                        "required": ["action", "targetEntityId", "key", "text", "reasoning", "sourceArtifactIds", "stances"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["positions"],
            "additionalProperties": false
        }),
        "synthesis_quality_assessment" => json!({
            "type": "object",
            "properties": {
                "classifications": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "eventId": { "type": "string", "minLength": 1 },
                            "classification": { "type": "string", "enum": ["strengthened", "weakened", "new-tension", "redundant"] },
                            "reason": { "type": "string", "minLength": 1 },
                            "visibleReaction": { "type": "string", "minLength": 1 }
                        },
                        "required": ["eventId", "classification", "reason", "visibleReaction"],
                        "additionalProperties": false
                    }
                },
                "depth": { "type": "string", "enum": ["selective", "full"] },
                "routingReason": { "type": "string", "minLength": 1 }
            },
            "required": ["classifications", "depth", "routingReason"],
            "additionalProperties": false
        }),
        "society_newspaper_sections" => {
            let entry = json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "minLength": 1, "pattern": "^[^\\r\\n]+$" },
                    "postReferences": {
                        "type": "array",
                        "minItems": 1,
                        "items": { "type": "string", "minLength": 2 }
                    }
                },
                "required": ["text", "postReferences"],
                "additionalProperties": false
            });
            json!({
                "type": "object",
                "properties": {
                    "sourceRecord": { "type": "array", "items": entry.clone() },
                    "synthesisInterpretation": { "type": "array", "items": entry.clone() },
                    "openQuestions": { "type": "array", "items": entry }
                },
                "required": ["sourceRecord", "synthesisInterpretation", "openQuestions"],
                "additionalProperties": false
            })
        }
        other => return Err(anyhow::anyhow!("unsupported JSON schema contract: {other}")),
    };
    Ok(ResponseJsonSchema {
        name: name.to_string(),
        schema,
    })
}

fn text_edit_tools() -> Vec<ResponseFunctionTool> {
    vec![
        ResponseFunctionTool {
            name: "replace_exact".to_string(),
            description: "Replace one exact, uniquely occurring string in the current report.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "old": { "type": "string", "minLength": 1 },
                    "new": { "type": "string" }
                },
                "required": ["old", "new"],
                "additionalProperties": false
            }),
        },
        ResponseFunctionTool {
            name: "insert_after_anchor".to_string(),
            description: "Insert text immediately after one exact, uniquely occurring anchor in the current report.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "anchor": { "type": "string", "minLength": 1 },
                    "text": { "type": "string", "minLength": 1 }
                },
                "required": ["anchor", "text"],
                "additionalProperties": false
            }),
        },
    ]
}

pub fn definitions_json() -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(AGENT_DEFINITIONS)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fugue_store::{
        BoardPostKind, ChangeClassification, EntityStateChange, NewspaperPostBody,
        NewspaperPostSource, NewspaperSections, SocietyNewspaperContext, SynthesisDoc,
        SynthesisMode, SynthesisReport, SynthesisSource,
    };

    fn synthesis_context() -> SynthesisContext {
        SynthesisContext {
            space_id: "space-1".to_string(),
            title: "Question".to_string(),
            compiled_document: SynthesisDoc {
                space_id: "space-1".to_string(),
                title: "Question".to_string(),
                version: 1,
                compiled_at: "now".to_string(),
                nodes: vec![],
            },
            sources: vec![SynthesisSource {
                artifact_id: "source-1".to_string(),
                input_kind: "lesswrongThread".to_string(),
                content: "Source text".to_string(),
                author_name: Some("Author".to_string()),
                source_url: Some("https://example.com/source".to_string()),
            }],
            causes: vec![ReportCause {
                event_id: "event-1".to_string(),
                kind: "entity_state_changed".to_string(),
                label: "Changed state".to_string(),
                source: None,
                author_name: None,
                unverified_display_name: None,
                objection_anchor: None,
                state_change: Some(EntityStateChange {
                    old_state: "open".to_string(),
                    new_state: "resolved".to_string(),
                }),
                input_text: Some("Position".to_string()),
                attributed_claim: None,
                change_classification: Some(ChangeClassification::Weakened),
                classification_reason: Some("resolved".to_string()),
            }],
            current_report: Some(SynthesisReport {
                artifact_id: "report-1".to_string(),
                space_id: "space-1".to_string(),
                version: 1,
                content: "Report".to_string(),
                mode: SynthesisMode::Full,
                routing_reason: None,
                created_at: "now".to_string(),
                provenance_anchors: vec![],
            }),
        }
    }

    #[test]
    fn classifier_remains_narrow_and_carries_state_delta() {
        let request = classifier(&synthesis_context()).unwrap();
        let input: Value = serde_json::from_str(&request.request.input).unwrap();
        let keys = input
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        assert_eq!(keys, vec!["currentReport", "newAcceptedInputs"]);
        assert_eq!(
            input["newAcceptedInputs"][0]["stateChange"]["oldState"],
            "open"
        );
        assert_eq!(
            input["newAcceptedInputs"][0]["stateChange"]["newState"],
            "resolved"
        );
        assert!(input.get("compiledDocument").is_none());
        assert!(input.get("sources").is_none());
    }

    #[test]
    fn generation_source_and_selective_payloads_follow_declarations() {
        let context = synthesis_context();
        let full = full_generation(&context).unwrap();
        let full_input: Value = serde_json::from_str(&full.request.input).unwrap();
        assert_eq!(full_input["sources"][0]["authorName"], "Author");
        assert_eq!(
            full_input["sources"][0]["sourceUrl"],
            "https://example.com/source"
        );
        assert!(full_input.get("currentReport").is_some());

        let selective = selective_edit(&context, "targeted change").unwrap();
        let selective_input: Value = serde_json::from_str(&selective.request.input).unwrap();
        assert!(selective_input.get("provenanceSources").is_none());
        assert_eq!(selective_input["currentReport"], "Report");
        assert!(selective.request.require_tool_call);
        assert_eq!(selective.request.tools.len(), 2);
    }

    #[test]
    fn validation_feedback_is_added_to_the_original_typed_context() {
        let request = classifier(&synthesis_context()).unwrap();
        let corrected = with_validation_feedback(request.request, "missing event event-1").unwrap();
        let input: Value = serde_json::from_str(&corrected.input).unwrap();
        assert_eq!(
            input["validationFeedback"]["error"],
            "missing event event-1"
        );
        assert!(input.get("currentReport").is_some());
        assert!(input.get("newAcceptedInputs").is_some());
    }

    #[test]
    fn newspaper_context_contains_only_current_sections_and_incremental_public_posts() {
        let context = SocietyNewspaperContext {
            space_id: "space-1".to_string(),
            space_name: "Commons".to_string(),
            base_artifact_id: Some("newspaper-1".to_string()),
            current_version: Some(1),
            current_sections: Some(NewspaperSections {
                source_record: vec![],
                synthesis_interpretation: vec![],
                open_questions: vec![],
            }),
            new_public_posts: vec![NewspaperPostSource {
                sequence: 7,
                reference: "P7".to_string(),
                author_address: "alice".to_string(),
                kind: BoardPostKind::Finding,
                headline: "Public finding".to_string(),
                body: NewspaperPostBody::Text {
                    text: "Source text".to_string(),
                },
                parent_reference: None,
                created_at: "now".to_string(),
            }],
            through_post_sequence: 6,
        };
        let request = society_newspaper(&context).unwrap();
        let input: Value = serde_json::from_str(&request.request.input).unwrap();
        assert_eq!(
            input.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec![
                "current",
                "newPublicPosts",
                "requiredNewReferences",
                "space"
            ]
        );
        assert_eq!(input["requiredNewReferences"], json!(["P7"]));
        assert_eq!(input["newPublicPosts"][0]["body"]["text"], "Source text");
        assert_eq!(
            request.request.json_schema.as_ref().unwrap().name,
            "society_newspaper_sections"
        );
    }

    #[test]
    fn exactly_seven_agents_are_declared_with_explicit_blocks() {
        assert_eq!(AGENT_DEFINITIONS.len(), 7);
        assert!(
            AGENT_DEFINITIONS
                .iter()
                .all(|agent| !agent.blocks.is_empty())
        );
    }
}
