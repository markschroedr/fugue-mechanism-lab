alter table actor add column address text;
alter table actor add column name text;
alter table actor add column description text;

create unique index actor_society_address_unique
    on actor(address)
    where address is not null;

create unique index space_society_slug_unique
    on space(json_extract(settings_json, '$.society.slug'))
    where json_extract(settings_json, '$.society.slug') is not null;
