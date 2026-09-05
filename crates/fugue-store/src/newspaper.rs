use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct NewspaperEntry {
    pub text: String,
    pub post_references: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct NewspaperSections {
    pub source_record: Vec<NewspaperEntry>,
    pub synthesis_interpretation: Vec<NewspaperEntry>,
    pub open_questions: Vec<NewspaperEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[specta(export)]
pub struct SocietyNewspaper {
    pub artifact_id: String,
    pub space_id: String,
    pub version: i64,
    pub content: String,
    pub sections: NewspaperSections,
    pub through_post_sequence: i64,
    pub through_post_reference: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum NewspaperPostBody {
    Text {
        text: String,
    },
    Link {
        url: String,
    },
    File {
        filename: String,
        media_type: String,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NewspaperPostSource {
    pub sequence: i64,
    pub reference: String,
    pub author_address: String,
    pub kind: BoardPostKind,
    pub headline: String,
    pub body: NewspaperPostBody,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_reference: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SocietyNewspaperContext {
    pub space_id: String,
    pub space_name: String,
    pub base_artifact_id: Option<String>,
    pub current_version: Option<i64>,
    pub current_sections: Option<NewspaperSections>,
    pub new_public_posts: Vec<NewspaperPostSource>,
    pub through_post_sequence: i64,
}

impl StoreInfo {
    pub fn enqueue_society_newspaper(
        &self,
        actor_id: &str,
        space_id: &str,
    ) -> anyhow::Result<WorkerJobReceipt> {
        validate_non_empty("space_id", space_id)?;
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_board_space(&transaction, space_id)?;
        let receipt = enqueue_newspaper_job(&transaction, actor_id, space_id, false)?;
        transaction.commit()?;
        Ok(receipt)
    }

    pub fn read_society_newspaper(
        &self,
        space_id: &str,
    ) -> anyhow::Result<Option<SocietyNewspaper>> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        ensure_board_space(&connection, space_id)?;
        read_society_newspaper_model(&connection, space_id)
    }

    pub fn build_society_newspaper_context(
        &self,
        space_id: &str,
    ) -> anyhow::Result<SocietyNewspaperContext> {
        validate_non_empty("space_id", space_id)?;
        let connection = self.open_workspace_connection()?;
        let space_name = ensure_board_space(&connection, space_id)?;
        let current = read_society_newspaper_model(&connection, space_id)?;
        let through_post_sequence = current
            .as_ref()
            .map(|newspaper| newspaper.through_post_sequence)
            .unwrap_or_default();
        let mut statement = connection.prepare(
            "select post.id
             from operations.society_post post
             where post.space_id = ?1 and post.sequence > ?2
               and not exists (
                   select 1 from operations.society_post_recipient recipient
                   where recipient.post_id = post.id
               )
             order by post.sequence",
        )?;
        let post_ids = statement
            .query_map(params![space_id, through_post_sequence], |row| {
                row.get::<_, String>(0)
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let new_public_posts = post_ids
            .iter()
            .map(|post_id| read_newspaper_post_source(&connection, post_id))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(SocietyNewspaperContext {
            space_id: space_id.to_string(),
            space_name,
            base_artifact_id: current
                .as_ref()
                .map(|newspaper| newspaper.artifact_id.clone()),
            current_version: current.as_ref().map(|newspaper| newspaper.version),
            current_sections: current.map(|newspaper| newspaper.sections),
            new_public_posts,
            through_post_sequence,
        })
    }

    pub fn committed_society_newspaper_effect(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<SocietyNewspaper>> {
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
            .map(|artifact_id| read_society_newspaper_artifact(&connection, &artifact_id))
            .transpose()
    }

    pub fn write_society_newspaper(
        &self,
        job_id: &str,
        context: &SocietyNewspaperContext,
        sections: NewspaperSections,
    ) -> anyhow::Result<SocietyNewspaper> {
        validate_non_empty("job_id", job_id)?;
        if let Some(newspaper) = self.committed_society_newspaper_effect(job_id)? {
            return Ok(newspaper);
        }
        validate_society_newspaper_sections(context, &sections)?;
        let through_post_sequence = context
            .new_public_posts
            .last()
            .map(|post| post.sequence)
            .ok_or_else(|| {
                StoreError::Validation(
                    "a Newspaper write requires at least one new public post".to_string(),
                )
            })?;
        let through_post_reference = format!("P{through_post_sequence}");
        let mut connection = self.open_workspace_connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let latest = read_society_newspaper_model(&transaction, &context.space_id)?;
        let latest_artifact_id = latest
            .as_ref()
            .map(|newspaper| newspaper.artifact_id.as_str());
        if latest_artifact_id != context.base_artifact_id.as_deref() {
            return Err(StoreError::Validation(
                "the Newspaper changed after this update started".to_string(),
            )
            .into());
        }
        let version = latest
            .as_ref()
            .map(|newspaper| newspaper.version + 1)
            .unwrap_or(1);
        let artifact_id = new_id("artifact");
        let created_at = now_label();
        let content = render_society_newspaper(
            &context.space_name,
            version,
            &through_post_reference,
            &sections,
        );
        ensure_system_actors(&transaction)?;
        transaction.execute(
            "insert into artifact
                (id, kind, media_type, content, derived_from, metadata_json)
             values (?1, 'derived', 'text/markdown', ?2, ?3, ?4)",
            params![
                artifact_id,
                content,
                context.base_artifact_id,
                json!({
                    "artifactType": "societyNewspaper",
                    "spaceId": context.space_id,
                    "newspaperJobId": job_id,
                    "newspaperVersion": version,
                    "throughPostSequence": through_post_sequence,
                    "createdAt": created_at,
                    "sections": sections,
                })
                .to_string(),
            ],
        )?;
        append_event(
            &transaction,
            Some(AGENT_ACTOR_ID),
            Some(&artifact_id),
            "society_newspaper_written",
            json!({
                "spaceId": context.space_id,
                "version": version,
                "throughPostReference": through_post_reference,
                "newspaperJobId": job_id,
                "previousArtifactId": context.base_artifact_id,
            }),
        )?;
        let changed = transaction.execute(
            "update operations.worker_job
             set effect_artifact_id = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where id = ?1 and effect_artifact_id is null",
            params![job_id, artifact_id],
        )?;
        if changed != 1 {
            return Err(StoreError::Validation(
                "Newspaper job effect was already committed".to_string(),
            )
            .into());
        }
        transaction.commit()?;
        Ok(SocietyNewspaper {
            artifact_id,
            space_id: context.space_id.clone(),
            version,
            content,
            sections,
            through_post_sequence,
            through_post_reference,
            created_at,
        })
    }
}

pub fn validate_society_newspaper_sections(
    context: &SocietyNewspaperContext,
    sections: &NewspaperSections,
) -> anyhow::Result<()> {
    let entries = sections
        .source_record
        .iter()
        .chain(&sections.synthesis_interpretation)
        .chain(&sections.open_questions)
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err(StoreError::Validation(
            "the Newspaper must contain at least one entry".to_string(),
        )
        .into());
    }
    let allowed = context
        .current_sections
        .iter()
        .flat_map(section_entries)
        .flat_map(|entry| entry.post_references.iter().cloned())
        .chain(
            context
                .new_public_posts
                .iter()
                .map(|post| post.reference.clone()),
        )
        .collect::<BTreeSet<_>>();
    let mut cited = BTreeSet::new();
    for entry in entries {
        if entry.text.trim().is_empty()
            || entry.text.trim() != entry.text
            || entry.text.contains(['\n', '\r'])
        {
            return Err(StoreError::Validation(
                "each Newspaper entry must be one trimmed, non-empty paragraph".to_string(),
            )
            .into());
        }
        if entry.post_references.is_empty() {
            return Err(StoreError::Validation(
                "each Newspaper entry must cite at least one post".to_string(),
            )
            .into());
        }
        let mut entry_references = BTreeSet::new();
        for reference in &entry.post_references {
            if !allowed.contains(reference) {
                return Err(StoreError::Validation(format!(
                    "Newspaper entry cites unknown or non-public post {reference}"
                ))
                .into());
            }
            if !entry_references.insert(reference) {
                return Err(StoreError::Validation(format!(
                    "Newspaper entry cites {reference} more than once"
                ))
                .into());
            }
            cited.insert(reference.clone());
        }
    }
    if let Some(missing) = context
        .new_public_posts
        .iter()
        .map(|post| post.reference.as_str())
        .find(|reference| !cited.contains(*reference))
    {
        return Err(StoreError::Validation(format!(
            "Newspaper update omitted new public post {missing}"
        ))
        .into());
    }
    Ok(())
}

pub(crate) fn enqueue_newspaper_job(
    connection: &Connection,
    actor_id: &str,
    space_id: &str,
    auto: bool,
) -> anyhow::Result<WorkerJobReceipt> {
    if let Some((job_id, existing_auto)) = connection
        .query_row(
            "select id, coalesce(json_extract(payload_json, '$.auto'), 0)
             from operations.worker_job
             where kind = 'society_newspaper' and state = 'queued'
               and effect_artifact_id is null and target_id = ?1
             order by created_at, rowid limit 1",
            params![space_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)),
        )
        .optional()?
    {
        let remains_automatic = existing_auto && auto;
        connection.execute(
            "update operations.worker_job
             set payload_json = ?2,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             where id = ?1",
            params![
                job_id,
                json!({
                    "spaceId": space_id,
                    "requestedBy": actor_id,
                    "auto": remains_automatic,
                })
                .to_string(),
            ],
        )?;
        return Ok(WorkerJobReceipt { job_id });
    }
    let job_id = new_id("job");
    connection.execute(
        "insert into operations.worker_job
            (id, kind, state, payload_json, target_id, created_at, updated_at)
         values (?1, 'society_newspaper', 'queued', ?2, ?3,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            job_id,
            json!({
                "spaceId": space_id,
                "requestedBy": actor_id,
                "auto": auto,
            })
            .to_string(),
            space_id,
        ],
    )?;
    Ok(WorkerJobReceipt { job_id })
}

pub(crate) fn read_society_newspaper_model(
    connection: &Connection,
    space_id: &str,
) -> anyhow::Result<Option<SocietyNewspaper>> {
    let artifact_id = connection
        .query_row(
            "select id from artifact
             where kind = 'derived'
               and json_extract(metadata_json, '$.artifactType') = 'societyNewspaper'
               and json_extract(metadata_json, '$.spaceId') = ?1
             order by cast(json_extract(metadata_json, '$.newspaperVersion') as integer) desc,
                      rowid desc
             limit 1",
            params![space_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    artifact_id
        .map(|artifact_id| read_society_newspaper_artifact(connection, &artifact_id))
        .transpose()
}

fn read_society_newspaper_artifact(
    connection: &Connection,
    artifact_id: &str,
) -> anyhow::Result<SocietyNewspaper> {
    connection
        .query_row(
            "select content, metadata_json from artifact where id = ?1",
            params![artifact_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("Newspaper artifact not found".to_string()))
        .and_then(|(content, metadata_json)| {
            let metadata: Value = serde_json::from_str(&metadata_json).map_err(|error| {
                StoreError::Validation(format!("invalid Newspaper metadata: {error}"))
            })?;
            if metadata.pointer("/artifactType").and_then(Value::as_str) != Some("societyNewspaper")
            {
                return Err(StoreError::Validation(
                    "worker effect is not a Newspaper artifact".to_string(),
                ));
            }
            let required_string = |pointer: &str, label: &str| {
                metadata
                    .pointer(pointer)
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .ok_or_else(|| {
                        StoreError::Validation(format!("Newspaper metadata is missing {label}"))
                    })
            };
            let required_integer = |pointer: &str, label: &str| {
                metadata
                    .pointer(pointer)
                    .and_then(Value::as_i64)
                    .ok_or_else(|| {
                        StoreError::Validation(format!("Newspaper metadata is missing {label}"))
                    })
            };
            let through_post_sequence =
                required_integer("/throughPostSequence", "throughPostSequence")?;
            let sections =
                serde_json::from_value(metadata.pointer("/sections").cloned().ok_or_else(
                    || StoreError::Validation("Newspaper metadata is missing sections".to_string()),
                )?)
                .map_err(|error| {
                    StoreError::Validation(format!("invalid Newspaper sections: {error}"))
                })?;
            Ok(SocietyNewspaper {
                artifact_id: artifact_id.to_string(),
                space_id: required_string("/spaceId", "spaceId")?,
                version: required_integer("/newspaperVersion", "newspaperVersion")?,
                content,
                sections,
                through_post_sequence,
                through_post_reference: format!("P{through_post_sequence}"),
                created_at: required_string("/createdAt", "createdAt")?,
            })
        })
        .map_err(Into::into)
}

fn ensure_board_space(connection: &Connection, space_id: &str) -> anyhow::Result<String> {
    connection
        .query_row(
            "select json_extract(settings_json, '$.title') from space
             where id = ?1 and json_extract(settings_json, '$.society.slug') is not null",
            params![space_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::NotFound("board space not found".to_string()).into())
}

fn read_newspaper_post_source(
    connection: &Connection,
    post_id: &str,
) -> anyhow::Result<NewspaperPostSource> {
    let post = read_board_post(connection, post_id, None)?;
    let body = match post
        .body
        .ok_or_else(|| anyhow::anyhow!("public board post body is unavailable"))?
    {
        BoardPostBody::Text { text } => NewspaperPostBody::Text { text },
        BoardPostBody::Link { url } => NewspaperPostBody::Link { url },
        BoardPostBody::File {
            filename,
            media_type,
            ..
        } => NewspaperPostBody::File {
            filename,
            media_type,
        },
    };
    let parent_reference = match post.parent_post_id.as_deref() {
        Some(parent_id) => connection
            .query_row(
                "select parent.sequence from operations.society_post parent
                 where parent.id = ?1 and not exists (
                     select 1 from operations.society_post_recipient recipient
                     where recipient.post_id = parent.id
                 )",
                params![parent_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .map(|sequence| format!("P{sequence}")),
        None => None,
    };
    Ok(NewspaperPostSource {
        sequence: post.sequence,
        reference: post.reference,
        author_address: post.author_address,
        kind: post.kind,
        headline: post.headline,
        body,
        parent_reference,
        created_at: post.created_at,
    })
}

fn section_entries(sections: &NewspaperSections) -> impl Iterator<Item = &NewspaperEntry> {
    sections
        .source_record
        .iter()
        .chain(&sections.synthesis_interpretation)
        .chain(&sections.open_questions)
}

fn render_society_newspaper(
    space_name: &str,
    version: i64,
    through_post_reference: &str,
    sections: &NewspaperSections,
) -> String {
    let title = space_name.replace(['\r', '\n'], " ");
    format!(
        "# {title} Newspaper\n\n_Version {version} · through {through_post_reference}_\n\n## Source record\n\n{}\n\n## Synthesis interpretation\n\n{}\n\n## Open questions\n\n{}\n",
        render_entries(&sections.source_record),
        render_entries(&sections.synthesis_interpretation),
        render_entries(&sections.open_questions),
    )
}

fn render_entries(entries: &[NewspaperEntry]) -> String {
    if entries.is_empty() {
        return "_No entries._".to_string();
    }
    entries
        .iter()
        .map(|entry| format!("- {} [{}]", entry.text, entry.post_references.join(", ")))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newspaper_advances_only_through_cited_public_posts() {
        let root = std::env::temp_dir().join(format!(
            "fugue-store-newspaper-{}-{}",
            std::process::id(),
            ID_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let store = initialize(&root).expect("store should initialize");
        let alice = store
            .create_society_agent("alice", "alice-key")
            .expect("Alice should exist");
        store
            .create_society_agent("bob", "bob-key")
            .expect("Bob should exist");
        let space = store
            .create_board_space(
                &alice.actor_id,
                CreateBoardSpaceInput {
                    slug: "commons".to_string(),
                    name: "Commons".to_string(),
                },
            )
            .expect("Commons should exist");
        let private = store
            .create_board_post(
                &alice.actor_id,
                CreateBoardPostInput {
                    space_id: space.id.clone(),
                    kind: BoardPostKind::Note,
                    headline: "Private".to_string(),
                    body: BoardPostBody::Text {
                        text: "never publish this".to_string(),
                    },
                    parent_post_id: None,
                    recipient_addresses: vec!["bob".to_string()],
                    attached_obols: 0,
                },
            )
            .expect("addressed post should commit");
        let public = store
            .create_board_post(
                &alice.actor_id,
                CreateBoardPostInput {
                    space_id: space.id.clone(),
                    kind: BoardPostKind::Finding,
                    headline: "Public finding".to_string(),
                    body: BoardPostBody::Text {
                        text: "Public evidence".to_string(),
                    },
                    parent_post_id: Some(private.id),
                    recipient_addresses: vec![],
                    attached_obols: 0,
                },
            )
            .expect("public post should commit");

        let context = store
            .build_society_newspaper_context(&space.id)
            .expect("Newspaper context should build");
        assert_eq!(
            context
                .new_public_posts
                .iter()
                .map(|post| post.reference.as_str())
                .collect::<Vec<_>>(),
            vec![public.reference.as_str()]
        );
        assert_eq!(context.new_public_posts[0].parent_reference, None);
        assert!(
            serde_json::to_string(&context)
                .expect("context should serialize")
                .contains("Public evidence")
        );
        assert!(
            !serde_json::to_string(&context)
                .expect("context should serialize")
                .contains("never publish this")
        );

        let sections = NewspaperSections {
            source_record: vec![NewspaperEntry {
                text: "Alice reports public evidence.".to_string(),
                post_references: vec![public.reference.clone()],
            }],
            synthesis_interpretation: vec![],
            open_questions: vec![],
        };
        let job = store
            .enqueue_society_newspaper(&alice.actor_id, &space.id)
            .expect("manual update should reuse the automatic job");
        let newspaper = store
            .write_society_newspaper(&job.job_id, &context, sections)
            .expect("a cited public update should commit");
        assert_eq!(newspaper.version, 1);
        assert_eq!(newspaper.through_post_reference, public.reference);
        assert!(newspaper.content.contains("## Source record"));
        assert!(!newspaper.content.contains("never publish this"));

        let next = store
            .create_board_post(
                &alice.actor_id,
                CreateBoardPostInput {
                    space_id: space.id.clone(),
                    kind: BoardPostKind::Question,
                    headline: "Next public question".to_string(),
                    body: BoardPostBody::Text {
                        text: "What would change the result?".to_string(),
                    },
                    parent_post_id: None,
                    recipient_addresses: vec![],
                    attached_obols: 0,
                },
            )
            .expect("next public post should commit");
        let next_context = store
            .build_society_newspaper_context(&space.id)
            .expect("incremental context should build");
        assert_eq!(next_context.current_version, Some(1));
        assert_eq!(next_context.new_public_posts.len(), 1);
        assert_eq!(next_context.new_public_posts[0].reference, next.reference);

        let invalid = NewspaperSections {
            source_record: vec![NewspaperEntry {
                text: "A private claim escaped.".to_string(),
                post_references: vec![private.reference],
            }],
            synthesis_interpretation: vec![],
            open_questions: vec![],
        };
        assert!(
            validate_society_newspaper_sections(&next_context, &invalid)
                .unwrap_err()
                .to_string()
                .contains("unknown or non-public")
        );

        std::fs::remove_dir_all(root).expect("test data should be removed");
    }
}
