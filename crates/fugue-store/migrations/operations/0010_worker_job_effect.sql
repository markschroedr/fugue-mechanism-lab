alter table worker_job add column effect_artifact_id text;

create unique index idx_worker_job_synthesis_effect
on worker_job(effect_artifact_id)
where effect_artifact_id is not null;
