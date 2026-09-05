create table thread (
    id text primary key,
    owner_id text,
    title text,
    state text not null
);

create table message (
    id text primary key,
    thread_id text not null,
    actor_id text,
    role text not null,
    content text not null,
    created_at text not null
);

create table attention_item (
    id text primary key,
    owner_id text,
    kind text not null,
    target_id text,
    state text not null,
    payload_json text not null
);

create table worker_job (
    id text primary key,
    kind text not null,
    state text not null,
    payload_json text not null,
    target_id text
);

create table worker_attempt (
    id text primary key,
    job_id text not null,
    state text not null,
    summary text
);

create table checkpoint (
    id text primary key,
    consumer text not null,
    source text not null,
    cursor text not null
);
