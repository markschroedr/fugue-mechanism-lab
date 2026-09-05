drop index if exists idx_worker_job_lesswrong_fetch_target;

create unique index idx_worker_job_lesswrong_fetch_target
on worker_job(target_id)
where kind = 'lesswrong_thread_fetch' and state in ('queued', 'running');
