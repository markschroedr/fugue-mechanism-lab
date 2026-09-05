alter table society_usage add column cost_usd_exact text;

drop trigger society_wake_debit_no_update;
drop trigger society_wake_debit_no_delete;

alter table society_wake_debit rename to society_wake_debit_legacy;

create table society_wake_debit (
    actor_id text not null,
    wake_number integer not null check (wake_number > 0),
    priced_amount integer not null check (priced_amount >= 0),
    amount integer not null check (amount >= 0 and amount <= priced_amount),
    treasury_actor_id text not null,
    ledger_id text unique,
    created_at text not null,
    primary key (actor_id, wake_number),
    check ((amount = 0 and ledger_id is null) or (amount > 0 and ledger_id is not null))
);

insert into society_wake_debit
    (actor_id, wake_number, priced_amount, amount, treasury_actor_id, ledger_id, created_at)
select actor_id, wake_number, amount, amount, treasury_actor_id, ledger_id, created_at
from society_wake_debit_legacy;

drop table society_wake_debit_legacy;

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
