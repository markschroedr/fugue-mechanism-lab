create table society_usage (
    id text primary key,
    actor_id text not null,
    model text not null,
    input_tokens integer not null check (input_tokens >= 0),
    cached_input_tokens integer not null check (cached_input_tokens >= 0),
    output_tokens integer not null check (output_tokens >= 0),
    cost_usd real,
    created_at text not null
);

create index society_usage_actor on society_usage(actor_id, created_at, id);

create trigger society_usage_no_update
before update on society_usage
begin
    select raise(abort, 'society usage is append-only');
end;

create trigger society_usage_no_delete
before delete on society_usage
begin
    select raise(abort, 'society usage is append-only');
end;
