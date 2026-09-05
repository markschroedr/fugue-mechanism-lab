create index idx_artifact_synthesis_report
on artifact (
    kind,
    json_extract(metadata_json, '$.artifactType'),
    json_extract(metadata_json, '$.spaceId'),
    cast(json_extract(metadata_json, '$.reportVersion') as integer)
);

create index idx_artifact_space_input
on artifact (
    json_extract(metadata_json, '$.spaceId'),
    json_extract(metadata_json, '$.inputKind')
);
