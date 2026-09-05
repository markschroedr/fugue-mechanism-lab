create table society_wake_line (
    singleton integer primary key check (singleton = 1),
    text text not null,
    updated_at text not null
);
