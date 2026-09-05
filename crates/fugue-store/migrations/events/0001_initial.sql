create table event (
    id text primary key,
    actor_id text,
    target_id text,
    kind text not null,
    decision_state text not null,
    payload_json text not null,
    created_at text not null
);
