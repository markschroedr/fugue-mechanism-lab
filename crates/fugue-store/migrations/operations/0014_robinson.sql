alter table society_usage add column wake_number integer;
alter table society_usage add column cache_write_input_tokens integer not null default 0 check (cache_write_input_tokens >= 0);
alter table society_usage add column reasoning_output_tokens integer not null default 0 check (reasoning_output_tokens >= 0);

create unique index society_usage_actor_wake
    on society_usage(actor_id, wake_number)
    where wake_number is not null;

create table society_allowance_grant (
    actor_id text not null,
    window_start text not null,
    window_end text not null,
    amount integer not null check (amount > 0),
    ledger_id text not null unique,
    created_at text not null,
    primary key (actor_id, window_start)
);

create trigger society_allowance_grant_no_update
before update on society_allowance_grant
begin
    select raise(abort, 'society allowance grants are append-only');
end;

create trigger society_allowance_grant_no_delete
before delete on society_allowance_grant
begin
    select raise(abort, 'society allowance grants are append-only');
end;

create table society_wake_debit (
    actor_id text not null,
    wake_number integer not null check (wake_number > 0),
    amount integer not null check (amount > 0),
    treasury_actor_id text not null,
    ledger_id text not null unique,
    created_at text not null,
    primary key (actor_id, wake_number)
);

create trigger society_wake_debit_no_update
before update on society_wake_debit
begin
    select raise(abort, 'society wake debits are append-only');
end;

create trigger society_wake_debit_no_delete
before delete on society_wake_debit
begin
    select raise(abort, 'society wake debits are append-only');
end;

create table society_transcript (
    id text primary key,
    actor_id text not null,
    wake_number integer not null check (wake_number > 0),
    session_jsonl blob not null,
    byte_length integer not null check (byte_length >= 0),
    sha256 text not null,
    created_at text not null,
    unique (actor_id, wake_number)
);

create index society_transcript_actor on society_transcript(actor_id, wake_number);

create trigger society_transcript_no_update
before update on society_transcript
begin
    select raise(abort, 'society transcripts are append-only');
end;

create trigger society_transcript_no_delete
before delete on society_transcript
begin
    select raise(abort, 'society transcripts are append-only');
end;
