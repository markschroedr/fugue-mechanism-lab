create table obol_ledger (
    id text primary key,
    kind text not null check (kind in ('mint', 'transfer', 'escrow_create', 'escrow_release', 'escrow_execute')),
    actor_id text,
    recipient_id text,
    escrow_id text,
    amount integer not null check (amount > 0),
    reference_id text,
    resolution_authority text check (resolution_authority in ('payer', 'recipient_read')),
    created_at text not null,
    check ((kind = 'escrow_create' and resolution_authority is not null) or
           (kind != 'escrow_create' and resolution_authority is null))
);

create index obol_ledger_actor on obol_ledger(actor_id, created_at, id);
create index obol_ledger_recipient on obol_ledger(recipient_id, created_at, id);
create index obol_ledger_escrow on obol_ledger(escrow_id, created_at, id);

create trigger obol_ledger_no_update
before update on obol_ledger
begin
    select raise(abort, 'obol ledger is append-only');
end;

create trigger obol_ledger_no_delete
before delete on obol_ledger
begin
    select raise(abort, 'obol ledger is append-only');
end;

create table society_post (
    sequence integer primary key autoincrement,
    id text not null unique,
    space_id text not null,
    author_actor_id text not null,
    kind text not null,
    headline text not null,
    artifact_id text not null,
    parent_post_id text references society_post(id),
    created_at text not null
);

create index society_post_space_sequence on society_post(space_id, sequence);
create index society_post_parent on society_post(parent_post_id, sequence);

create table society_post_recipient (
    post_id text not null references society_post(id),
    actor_id text not null,
    primary key (post_id, actor_id)
);

create index society_post_recipient_actor on society_post_recipient(actor_id, post_id);

create table society_attention (
    post_id text not null references society_post(id),
    recipient_actor_id text not null,
    amount integer not null check (amount >= 0),
    escrow_id text,
    primary key (post_id, recipient_actor_id),
    check ((amount = 0 and escrow_id is null) or (amount > 0 and escrow_id is not null))
);

create index society_attention_inbox on society_attention(recipient_actor_id, amount desc, post_id);

create table society_post_read (
    post_id text not null references society_post(id),
    actor_id text not null,
    read_at text not null,
    settlement_ledger_id text,
    primary key (post_id, actor_id)
);

create table society_read_cursor (
    actor_id text not null,
    space_id text not null,
    post_sequence integer not null,
    updated_at text not null,
    primary key (actor_id, space_id)
);
