create table society_allowance_expiry (
    actor_id text not null,
    window_start text not null,
    window_end text not null,
    granted_amount integer not null check (granted_amount > 0),
    spent_amount integer not null check (spent_amount >= 0),
    expired_amount integer not null check (expired_amount >= 0),
    treasury_actor_id text not null,
    ledger_id text unique,
    created_at text not null,
    primary key (actor_id, window_start),
    check ((expired_amount = 0 and ledger_id is null) or (expired_amount > 0 and ledger_id is not null))
);

create trigger society_allowance_expiry_no_update
before update on society_allowance_expiry
begin
    select raise(abort, 'society allowance expiries are append-only');
end;

create trigger society_allowance_expiry_no_delete
before delete on society_allowance_expiry
begin
    select raise(abort, 'society allowance expiries are append-only');
end;

create table society_agent_lifecycle (
    actor_id text primary key,
    phase text not null check (phase in ('awake', 'sleeping')),
    wake_number integer not null check (wake_number > 0),
    next_wake_at text,
    updated_at text not null,
    check ((phase = 'awake' and next_wake_at is null) or (phase = 'sleeping' and next_wake_at is not null))
);
