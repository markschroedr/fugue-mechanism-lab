pragma foreign_keys = off;

create table space_next (
    id text primary key,
    parent_id text references space(id),
    org_id text,
    settings_json text not null default '{}'
);

insert into space_next (id, parent_id, org_id, settings_json)
select id, parent_id, org_id, settings_json
from space;

drop table space;
alter table space_next rename to space;

create table actor_next (
    id text primary key,
    kind text not null,
    owner_id text references actor(id),
    keys_json text not null default '{}'
);

insert into actor_next (id, kind, owner_id, keys_json)
select id, kind, owner_id, keys_json
from actor;

drop table actor;
alter table actor_next rename to actor;

create table artifact_next (
    id text primary key,
    kind text not null,
    media_type text not null,
    content text,
    blob_path text,
    derived_from text references artifact(id),
    metadata_json text not null default '{}'
);

insert into artifact_next (id, kind, media_type, content, blob_path, derived_from, metadata_json)
select id, kind, media_type, content, blob_path, derived_from, metadata_json
from artifact;

drop table artifact;
alter table artifact_next rename to artifact;

create table entity_next (
    id text primary key,
    space_id text not null references space(id) on delete cascade,
    content text not null,
    role text not null check (role in ('issue', 'goal', 'action')),
    state text not null,
    created_by text references actor(id),
    confirmed_by text references actor(id),
    decay_at text
);

insert into entity_next (id, space_id, content, role, state, created_by, confirmed_by, decay_at)
select id, space_id, content, role, state, created_by, confirmed_by, decay_at
from entity
where space_id in (select id from space);

drop table entity;
alter table entity_next rename to entity;

create table contract_next (
    id text primary key,
    parties_json text not null,
    terms text not null,
    status text not null,
    locked_at text
);

insert into contract_next (id, parties_json, terms, status, locked_at)
select id, parties_json, terms, status, locked_at
from contract;

drop table contract;
alter table contract_next rename to contract;

create table relation_next (
    id text primary key,
    from_id text not null,
    to_id text not null,
    kind text not null,
    created_by text references actor(id),
    confirmed_by text references actor(id)
);

insert into relation_next (id, from_id, to_id, kind, created_by, confirmed_by)
select id, from_id, to_id, kind, created_by, confirmed_by
from relation;

drop table relation;
alter table relation_next rename to relation;

create table stance_next (
    id text primary key,
    actor_id text not null references actor(id),
    target_id text not null references entity(id) on delete cascade,
    axis text not null,
    value text not null,
    created_at text not null
);

insert into stance_next (id, actor_id, target_id, axis, value, created_at)
select id, actor_id, target_id, axis, value, created_at
from stance
where actor_id in (select id from actor)
  and target_id in (select id from entity);

drop table stance;
alter table stance_next rename to stance;

create table synthesis_doc (
    space_id text primary key references space(id) on delete cascade,
    title text not null,
    version integer not null,
    compiled_at text not null,
    nodes_json text not null
);

create index idx_space_parent on space(parent_id);
create index idx_entity_space_role on entity(space_id, role);
create index idx_relation_from_kind on relation(from_id, kind);
create index idx_relation_to_kind on relation(to_id, kind);
create index idx_stance_target_axis_created on stance(target_id, axis, created_at);

pragma foreign_keys = on;
