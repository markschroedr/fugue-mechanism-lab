create table worker_job_next (
    id text primary key,
    kind text not null,
    state text not null,
    payload_json text not null,
    target_id text
);

insert into worker_job_next (id, kind, state, payload_json, target_id)
select
    id,
    kind,
    state,
    payload_json,
    json_extract(payload_json, '$.annotation_id')
from worker_job;

drop table worker_job;
alter table worker_job_next rename to worker_job;
