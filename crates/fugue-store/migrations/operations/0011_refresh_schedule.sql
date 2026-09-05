create table refresh_schedule (
    space_id text primary key,
    cadence_seconds integer not null check (cadence_seconds > 0),
    next_due_at text not null,
    enabled integer not null check (enabled in (0, 1)),
    created_at text not null
);

create index idx_refresh_schedule_due
on refresh_schedule(next_due_at)
where enabled = 1;
