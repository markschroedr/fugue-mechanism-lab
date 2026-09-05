alter table stance add column evidence_artifact_id text references artifact(id);
alter table stance add column stated_confidence text;

-- Legacy harvested stances can be grounded only when exactly one authored relation
-- supports the actor/entity pair. Ambiguous rows stay unsupported and are omitted
-- from harvested-holder projections rather than receiving guessed evidence.
update stance
set evidence_artifact_id = (
    select min(artifact.id)
    from relation
    join artifact on artifact.id = relation.to_id
    where relation.from_id = stance.target_id
      and relation.kind = 'derived_from'
      and json_extract(artifact.metadata_json, '$.authorId') = stance.actor_id
      and julianday(json_extract(artifact.metadata_json, '$.postedAt')) is not null
      and instr(json_extract(artifact.metadata_json, '$.postedAt'), 'T') > 0
)
where stance.actor_id like 'lw-actor-%'
  and stance.rowid = (
    select max(latest.rowid)
    from stance as latest
    where latest.actor_id = stance.actor_id
      and latest.target_id = stance.target_id
      and latest.axis = stance.axis
  )
  and 1 = (
    select count(distinct artifact.id)
    from relation
    join artifact on artifact.id = relation.to_id
    where relation.from_id = stance.target_id
      and relation.kind = 'derived_from'
      and json_extract(artifact.metadata_json, '$.authorId') = stance.actor_id
      and julianday(json_extract(artifact.metadata_json, '$.postedAt')) is not null
      and instr(json_extract(artifact.metadata_json, '$.postedAt'), 'T') > 0
  );

create unique index idx_stance_exact_evidence
    on stance(actor_id, target_id, axis, evidence_artifact_id)
    where evidence_artifact_id is not null;
create index idx_stance_current on stance(target_id, axis, actor_id, created_at);
create index idx_stance_evidence on stance(evidence_artifact_id);
