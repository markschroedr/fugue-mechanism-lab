alter table worker_attempt add column llm_calls_json text not null default '[]';

create unique index idx_worker_job_one_queued_auto_synthesis
on worker_job(target_id)
where kind = 'synthesize'
  and state = 'queued'
  and json_extract(payload_json, '$.auto') = 1;
