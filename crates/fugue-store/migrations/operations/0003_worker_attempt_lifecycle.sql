pragma foreign_keys = off;

create table thread_next (
    id text primary key,
    owner_id text,
    title text,
    state text not null,
    anchor_block_id text,
    quote text,
    stance text check (stance is null or stance in ('believe', 'dispute')),
    created_at text not null default current_timestamp,
    updated_at text not null default current_timestamp
);

insert into thread_next (id, owner_id, title, state, created_at, updated_at)
select id, owner_id, title, state, current_timestamp, current_timestamp
from thread;

drop table thread;
alter table thread_next rename to thread;

create table message_next (
    id text primary key,
    thread_id text not null references thread(id) on delete cascade,
    actor_id text,
    role text not null check (role in ('user', 'agent')),
    content text not null,
    created_at text not null
);

insert into message_next (id, thread_id, actor_id, role, content, created_at)
select id, thread_id, actor_id, role, content, created_at
from message
where thread_id in (select id from thread);

drop table message;
alter table message_next rename to message;

create table attention_item_next (
    id text primary key,
    owner_id text,
    kind text not null check (kind in ('session', 'ingest', 'lint')),
    target_id text,
    state text not null,
    payload_json text not null,
    created_at text not null default current_timestamp,
    resolved_at text
);

insert into attention_item_next (id, owner_id, kind, target_id, state, payload_json, created_at)
select id, owner_id, kind, target_id, state, payload_json, current_timestamp
from attention_item;

drop table attention_item;
alter table attention_item_next rename to attention_item;

create table worker_job_next (
    id text primary key,
    kind text not null,
    state text not null check (state in ('queued', 'running', 'succeeded', 'failed', 'cancelled')),
    payload_json text not null,
    target_id text,
    created_at text not null default current_timestamp,
    updated_at text not null default current_timestamp,
    claimed_at text,
    finished_at text,
    error text
);

insert into worker_job_next (id, kind, state, payload_json, target_id, created_at, updated_at)
select id, kind, state, payload_json, target_id, current_timestamp, current_timestamp
from worker_job;

drop table worker_job;
alter table worker_job_next rename to worker_job;

create table worker_attempt_next (
    id text primary key,
    job_id text not null references worker_job(id) on delete cascade,
    state text not null check (state in ('running', 'succeeded', 'failed')),
    started_at text not null,
    finished_at text,
    error text,
    result_summary text
);

insert into worker_attempt_next (id, job_id, state, started_at, result_summary)
select worker_attempt.id, worker_attempt.job_id, worker_attempt.state, current_timestamp, worker_attempt.summary
from worker_attempt
join worker_job on worker_job.id = worker_attempt.job_id;

drop table worker_attempt;
alter table worker_attempt_next rename to worker_attempt;

create index idx_message_thread_created on message(thread_id, created_at);
create index idx_attention_item_state_created on attention_item(state, created_at);
create index idx_worker_job_state_created on worker_job(state, created_at);
create index idx_worker_attempt_job_started on worker_attempt(job_id, started_at);

pragma foreign_keys = on;
