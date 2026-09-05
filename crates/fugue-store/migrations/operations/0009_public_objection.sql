pragma foreign_keys = off;

create table attention_item_next (
    id text primary key,
    owner_id text,
    kind text not null check (kind in ('session', 'entity', 'ingest', 'objection', 'lint')),
    target_id text,
    state text not null,
    payload_json text not null,
    created_at text not null default current_timestamp,
    resolved_at text
);

insert into attention_item_next (id, owner_id, kind, target_id, state, payload_json, created_at, resolved_at)
select id, owner_id, kind, target_id, state, payload_json, created_at, resolved_at
from attention_item;

drop table attention_item;
alter table attention_item_next rename to attention_item;

create index idx_attention_item_state_created on attention_item(state, created_at);

pragma foreign_keys = on;
