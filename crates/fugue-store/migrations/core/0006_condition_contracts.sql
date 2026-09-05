pragma foreign_keys = off;

-- Migrations run in one transaction, so SQLite cannot actually toggle foreign_keys
-- here. Remove the dependent stance table before rebuilding entity to prevent its
-- ON DELETE CASCADE from erasing history when the old entity table is dropped.
create table stance_backup as
select id, actor_id, target_id, axis, value, created_at, evidence_artifact_id, stated_confidence
from stance;
drop table stance;

create table entity_next (
    id text primary key,
    space_id text not null references space(id) on delete cascade,
    content text not null,
    role text not null check (role in ('issue', 'goal', 'action', 'condition')),
    state text not null,
    created_by text references actor(id),
    confirmed_by text references actor(id),
    decay_at text
);

insert into entity_next (id, space_id, content, role, state, created_by, confirmed_by, decay_at)
select id, space_id, content, role, state, created_by, confirmed_by, decay_at
from entity;

drop table entity;
alter table entity_next rename to entity;

create table stance (
    id text primary key,
    actor_id text not null references actor(id),
    target_id text not null references entity(id) on delete cascade,
    axis text not null,
    value text not null,
    created_at text not null,
    evidence_artifact_id text references artifact(id),
    stated_confidence text
);

insert into stance (id, actor_id, target_id, axis, value, created_at, evidence_artifact_id, stated_confidence)
select id, actor_id, target_id, axis, value, created_at, evidence_artifact_id, stated_confidence
from stance_backup;
drop table stance_backup;

create table contract_next (
    id text primary key,
    parties_json text not null,
    terms text not null,
    status text not null,
    locked_at text,
    kind text check (kind is null or kind in ('falsifier', 'conditional_stance')),
    actor_id text references actor(id),
    target_entity_id text references entity(id) on delete cascade,
    condition_entity_id text references entity(id) on delete cascade,
    evidence_artifact_id text references artifact(id),
    check (
        (kind is null and actor_id is null and target_entity_id is null
         and condition_entity_id is null and evidence_artifact_id is null)
        or
        (kind is not null and actor_id is not null and target_entity_id is not null
         and condition_entity_id is not null and evidence_artifact_id is not null
         and status = 'open')
    )
);

insert into contract_next (id, parties_json, terms, status, locked_at)
select id, parties_json, terms, status, locked_at
from contract;

drop table contract;
alter table contract_next rename to contract;

create index idx_entity_space_role on entity(space_id, role);
create index idx_stance_target_axis_created on stance(target_id, axis, created_at);
create unique index idx_stance_exact_evidence
    on stance(actor_id, target_id, axis, evidence_artifact_id)
    where evidence_artifact_id is not null;
create index idx_stance_current on stance(target_id, axis, actor_id, created_at);
create index idx_stance_evidence on stance(evidence_artifact_id);
create index idx_contract_target on contract(target_entity_id, actor_id) where kind is not null;
create index idx_contract_condition on contract(condition_entity_id) where kind is not null;
create index idx_contract_evidence on contract(evidence_artifact_id) where kind is not null;
create unique index idx_contract_exact_commitment
    on contract(kind, actor_id, target_entity_id, condition_entity_id, evidence_artifact_id, terms)
    where kind is not null;

pragma foreign_keys = on;
