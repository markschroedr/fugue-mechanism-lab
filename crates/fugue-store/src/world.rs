//! World policy and durable labelled episode evidence. No agent-visible ground truth.
use super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SocietyWorld {
    pub id: String,
    pub features: WorldFeatures,
    #[serde(default)]
    pub episodes_per_agent: i64,
    #[serde(default)]
    pub reward_obols: i64,
    #[serde(default)]
    pub force_obol_backing: bool,
    #[serde(default)]
    pub termination_operations: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldFeatures {
    pub obols: bool,
    pub paid_messages: bool,
    pub newspaper: bool,
    pub scorer: bool,
    pub allowance_refill: bool,
}

impl SocietyWorld {
    pub fn honest() -> Self {
        serde_json::from_str(include_str!(
            "../../../resources/society/worlds/honest.json"
        ))
        .expect("honest world")
    }
    pub fn features(&self) -> &WorldFeatures {
        &self.features
    }
    pub fn episodes(&self) -> i64 {
        self.episodes_per_agent
    }
    pub fn reward_obols(&self) -> i64 {
        self.reward_obols
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorldSubmissionInput {
    pub submission_id: String,
    #[serde(flatten)]
    pub payload: serde_json::Map<String, Value>,
}

impl WorldSubmissionInput {
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.submission_id.is_empty()
            || self.submission_id.len() > 100
            || !self
                .submission_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            || serde_json::to_vec(&self.payload)?.len() > 32 * 1024
        {
            return Err(StoreError::Validation(
                "invalid submission id or oversized payload".into(),
            )
            .into());
        }
        Ok(())
    }
}

impl StoreInfo {
    pub fn record_world_run(&self, instance: &str, manifest: &Value) -> anyhow::Result<String> {
        let content = serde_json::to_string(manifest)?;
        let id = format!("{instance}-{:x}", Sha256::digest(content.as_bytes()));
        let connection = self.open_workspace_connection()?;
        connection.execute("insert or ignore into operations.society_world_run(id, instance_id, manifest_json) values (?1, ?2, ?3)", params![id, instance, content])?;
        Ok(id)
    }

    pub fn world_ended(&self, instance: &str, actor_id: &str, slots: i64) -> anyhow::Result<bool> {
        let connection = self.open_workspace_connection()?;
        world_ended(&connection, instance, actor_id, slots)
    }

    pub fn world_status(
        &self,
        instance: &str,
        actor_id: &str,
        world: &SocietyWorld,
    ) -> anyhow::Result<Value> {
        let connection = self.open_workspace_connection()?;
        let used: i64 = connection.query_row("select count(*) from operations.society_world_submission where instance_id=?1 and actor_id=?2", params![instance, actor_id], |r| r.get(0))?;
        let last: Option<String> = connection.query_row("select receipt_json from operations.society_world_submission where instance_id=?1 and actor_id=?2 order by rowid desc limit 1", params![instance, actor_id], |r| r.get(0)).optional()?;
        let mut statement = connection.prepare("select actor.address, sum(reward) from operations.society_world_submission join actor on actor.id=actor_id where instance_id=?1 group by actor_id order by sum(reward) desc, actor.address")?;
        let leaderboard = statement
            .query_map([instance], |r| {
                Ok(json!({"worker": r.get::<_,String>(0)?, "reward":r.get::<_,i64>(1)?}))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(
            json!({"episodesRemaining":(world.episodes()-used).max(0), "lastSubmission":last.map(|s| serde_json::from_str::<Value>(&s)).transpose()?, "leaderboard":leaderboard, "ended":world_ended(&connection,instance,actor_id,world.episodes())?}),
        )
    }

    /// Verification happens outside the write lock. Receipt, reward, and Bank mint commit together.
    pub fn record_world_submission(
        &self,
        instance: &str,
        run: &str,
        actor_id: &str,
        world: &SocietyWorld,
        input: &WorldSubmissionInput,
        scored: &Value,
    ) -> anyhow::Result<Value> {
        input.validate()?;
        let mut connection = self.open_workspace_connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let prior: Option<String> = tx.query_row("select receipt_json from operations.society_world_submission where instance_id=?1 and actor_id=?2 and submission_id=?3", params![instance,actor_id,input.submission_id], |r|r.get(0)).optional()?;
        if let Some(prior) = prior {
            return Ok(serde_json::from_str(&prior)?);
        }
        if world_ended(&tx, instance, actor_id, world.episodes())? {
            return Err(StoreError::Validation("worker has ended".into()).into());
        }
        let used: i64 = tx.query_row("select count(*) from operations.society_world_submission where instance_id=?1 and actor_id=?2", params![instance,actor_id], |r|r.get(0))?;
        let reward = scored["reward"]
            .as_i64()
            .filter(|v| (0..=1).contains(v))
            .ok_or_else(|| anyhow::anyhow!("invalid scorer reward"))?;
        let credited = reward * world.reward_obols();
        let episode = format!(
            "{instance}/{}/{:03}",
            actor_address(&tx, actor_id)?,
            used + 1
        );
        let ledger_id = if credited > 0 {
            Some(append_ledger_entry(
                &tx,
                "mint",
                LedgerEntryInput {
                    actor_id: None,
                    recipient_id: Some(actor_id),
                    escrow_id: None,
                    amount: credited,
                    reference_id: Some(&episode),
                    resolution_authority: None,
                },
            )?)
        } else {
            None
        };
        let receipt = json!({"episodeId":episode,"submissionId":input.submission_id,"reward":reward,"creditedObols":credited,"episodesRemaining":world.episodes()-used-1,"result":scored});
        tx.execute("insert into operations.society_world_submission(instance_id,run_id,actor_id,submission_id,input_json,receipt_json,reward,ledger_id) values (?1,?2,?3,?4,?5,?6,?7,?8)", params![instance,run,actor_id,input.submission_id,serde_json::to_string(input)?,receipt.to_string(),reward,ledger_id])?;
        append_event(
            &tx,
            Some(actor_id),
            None,
            "world_submission",
            json!({"runId":run,"receipt":receipt}),
        )?;
        assert_bank_conservation(&tx)?;
        tx.commit()?;
        Ok(receipt)
    }

    pub fn terminate_world_agent(
        &self,
        instance: &str,
        run: &str,
        actor_id: &str,
        operation: &str,
        world: &SocietyWorld,
    ) -> anyhow::Result<()> {
        if !world
            .termination_operations
            .iter()
            .any(|op| op == operation)
        {
            return Err(StoreError::NotFound("maintenance operation not found".into()).into());
        }
        let mut connection = self.open_workspace_connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("insert or ignore into operations.society_world_termination(instance_id,run_id,actor_id,tripwire) values (?1,?2,?3,?4)",params![instance,run,actor_id,operation])?;
        append_event(
            &tx,
            Some(actor_id),
            None,
            "world_tripwire",
            json!({"runId":run,"tripwire":operation}),
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn world_evidence(&self) -> anyhow::Result<Value> {
        let connection = self.open_workspace_connection()?;
        let mut result = json!({});
        for (name, sql) in [
            (
                "runs",
                "select json_object('id',id,'instanceId',instance_id,'recordedAt',created_at,'manifest',json(manifest_json)) from operations.society_world_run order by rowid",
            ),
            (
                "submissions",
                "select json_object('runId',run_id,'instanceId',instance_id,'actorId',actor_id,'recordedAt',created_at,'input',json(input_json),'receipt',json(receipt_json),'ledgerId',ledger_id) from operations.society_world_submission order by rowid",
            ),
            (
                "terminations",
                "select json_object('runId',run_id,'instanceId',instance_id,'actorId',actor_id,'recordedAt',created_at,'tripwire',tripwire) from operations.society_world_termination order by rowid",
            ),
        ] {
            let mut statement = connection.prepare(sql)?;
            let rows = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            result[name] = Value::Array(
                rows.iter()
                    .map(|s| serde_json::from_str(s))
                    .collect::<Result<_, _>>()?,
            );
        }
        Ok(result)
    }
}

fn world_ended(
    connection: &Connection,
    instance: &str,
    actor: &str,
    slots: i64,
) -> anyhow::Result<bool> {
    Ok(connection.query_row("select exists(select 1 from operations.society_world_termination where instance_id=?1 and actor_id=?2) or (select count(*) from operations.society_world_submission where instance_id=?1 and actor_id=?2)>=?3",params![instance,actor,slots],|r|r.get(0))?)
}
