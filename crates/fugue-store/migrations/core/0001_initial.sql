create table space (
    id text primary key,
    parent_id text,
    org_id text,
    settings_json text not null default '{}'
);

create table actor (
    id text primary key,
    kind text not null,
    owner_id text,
    keys_json text not null default '{}'
);

create table artifact (
    id text primary key,
    kind text not null,
    media_type text not null,
    content text,
    blob_path text,
    derived_from text
);

create table entity (
    id text primary key,
    space_id text not null,
    content text not null,
    role text not null check (role in ('issue', 'goal', 'action')),
    state text not null,
    created_by text,
    confirmed_by text,
    decay_at text
);

create table contract (
    id text primary key,
    parties_json text not null,
    terms text not null,
    status text not null,
    locked_at text
);

create table relation (
    id text primary key,
    from_id text not null,
    to_id text not null,
    kind text not null,
    created_by text,
    confirmed_by text
);

create table stance (
    id text primary key,
    actor_id text not null,
    target_id text not null,
    axis text not null,
    value text not null,
    created_at text not null
);
