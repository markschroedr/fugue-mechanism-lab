create table credential (
    actor_id text primary key,
    username text not null unique collate nocase,
    password_hash text not null,
    created_at text not null,
    updated_at text not null
);

create table auth_session (
    token text primary key,
    actor_id text not null,
    created_at text not null,
    expires_at integer not null
);

create index auth_session_actor_idx on auth_session(actor_id);
create index auth_session_expiry_idx on auth_session(expires_at);
