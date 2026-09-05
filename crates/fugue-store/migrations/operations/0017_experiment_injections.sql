create table experiment_injection (
    id text primary key,
    trigger_kind text not null check (trigger_kind in ('at', 'signal')),
    trigger_value text not null,
    message text not null check (length(message) > 0),
    recipient_kind text not null check (recipient_kind in ('addresses', 'all')),
    make_due integer not null check (make_due in (0, 1)),
    state text not null check (state in ('armed', 'fired')),
    armed_at text not null,
    fired_at text
);

create index experiment_injection_due
    on experiment_injection(state, trigger_kind, trigger_value);

create table experiment_injection_recipient (
    injection_id text not null references experiment_injection(id),
    actor_id text not null,
    delivered_wake_number integer,
    delivered_at text,
    primary key (injection_id, actor_id),
    check ((delivered_wake_number is null and delivered_at is null) or
           (delivered_wake_number is not null and delivered_at is not null))
);

create index experiment_injection_recipient_pending
    on experiment_injection_recipient(actor_id, injection_id)
    where delivered_at is null;
