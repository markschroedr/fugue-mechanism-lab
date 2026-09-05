use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use super::new_id;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct WakeGate {
    path: PathBuf,
    max_active: i64,
    lease_seconds: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakePermitRecord {
    pub token: String,
    pub lease_expires_at: i64,
}

impl WakeGate {
    pub fn open(
        path: impl AsRef<Path>,
        max_active: i64,
        lease_seconds: i64,
    ) -> anyhow::Result<Self> {
        if max_active < 1 || lease_seconds < 1 {
            anyhow::bail!("wake gate limit and lease must be positive");
        }
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create wake gate directory {}", parent.display())
            })?;
        }
        let gate = Self {
            path,
            max_active,
            lease_seconds,
        };
        let connection = gate.connection()?;
        connection.execute_batch(
            "pragma journal_mode = wal;
             create table if not exists wake_gate_config (
               singleton integer primary key check (singleton = 1),
               max_active integer not null check (max_active > 0)
             );
             create table if not exists wake_permit (
               instance_id text not null,
               actor_address text not null,
               wake_number integer not null check (wake_number > 0),
               token text not null unique,
               lease_expires_at integer not null,
               primary key (instance_id, actor_address)
             );",
        )?;
        connection.execute(
            "insert or ignore into wake_gate_config(singleton, max_active) values (1, ?1)",
            params![max_active],
        )?;
        let configured: i64 = connection.query_row(
            "select max_active from wake_gate_config where singleton = 1",
            [],
            |row| row.get(0),
        )?;
        if configured != max_active {
            anyhow::bail!(
                "wake gate {} is configured for {configured} active wakes, not {max_active}",
                gate.path.display()
            );
        }
        Ok(gate)
    }

    pub fn acquire(
        &self,
        instance_id: &str,
        actor_address: &str,
        wake_number: i64,
    ) -> anyhow::Result<Option<WakePermitRecord>> {
        if wake_number < 1 {
            anyhow::bail!("wakeNumber must be positive");
        }
        let now = unix_seconds()?;
        let expires = now + self.lease_seconds;
        let mut connection = self.connection()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "delete from wake_permit where lease_expires_at <= ?1",
            params![now],
        )?;
        let existing = transaction
            .query_row(
                "select wake_number, token from wake_permit
                 where instance_id = ?1 and actor_address = ?2",
                params![instance_id, actor_address],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        if let Some((existing_wake, _token)) = existing {
            if existing_wake != wake_number {
                anyhow::bail!(
                    "{actor_address} already holds a wake permit for wake {existing_wake}"
                );
            }
            transaction.commit()?;
            return Ok(None);
        }
        let active: i64 =
            transaction.query_row("select count(*) from wake_permit", [], |row| row.get(0))?;
        if active >= self.max_active {
            transaction.commit()?;
            return Ok(None);
        }
        let token = new_id("wake-permit");
        transaction.execute(
            "insert into wake_permit(
               instance_id, actor_address, wake_number, token, lease_expires_at
             ) values (?1, ?2, ?3, ?4, ?5)",
            params![instance_id, actor_address, wake_number, token, expires],
        )?;
        transaction.commit()?;
        Ok(Some(WakePermitRecord {
            token,
            lease_expires_at: expires,
        }))
    }

    pub fn heartbeat(
        &self,
        instance_id: &str,
        actor_address: &str,
        wake_number: i64,
        token: &str,
    ) -> anyhow::Result<Option<WakePermitRecord>> {
        let now = unix_seconds()?;
        let expires = now + self.lease_seconds;
        let connection = self.connection()?;
        let changed = connection.execute(
            "update wake_permit set lease_expires_at = ?1
             where instance_id = ?2 and actor_address = ?3 and wake_number = ?4
               and token = ?5 and lease_expires_at > ?6",
            params![expires, instance_id, actor_address, wake_number, token, now],
        )?;
        Ok((changed == 1).then(|| WakePermitRecord {
            token: token.to_string(),
            lease_expires_at: expires,
        }))
    }

    pub fn validate(
        &self,
        instance_id: &str,
        actor_address: &str,
        wake_number: i64,
        token: &str,
    ) -> anyhow::Result<bool> {
        let now = unix_seconds()?;
        let connection = self.connection()?;
        Ok(connection.query_row(
            "select exists(
                select 1 from wake_permit
                where instance_id = ?1 and actor_address = ?2 and wake_number = ?3
                  and token = ?4 and lease_expires_at > ?5
             )",
            params![instance_id, actor_address, wake_number, token, now],
            |row| row.get(0),
        )?)
    }

    pub fn release(
        &self,
        instance_id: &str,
        actor_address: &str,
        wake_number: i64,
        token: &str,
    ) -> anyhow::Result<bool> {
        let connection = self.connection()?;
        let existing = connection
            .query_row(
                "select wake_number, token from wake_permit
                 where instance_id = ?1 and actor_address = ?2",
                params![instance_id, actor_address],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((existing_wake, existing_token)) = existing else {
            return Ok(false);
        };
        if existing_wake != wake_number || existing_token != token {
            anyhow::bail!("wake permit does not match its active owner");
        }
        connection.execute(
            "delete from wake_permit where instance_id = ?1 and actor_address = ?2",
            params![instance_id, actor_address],
        )?;
        Ok(true)
    }

    fn connection(&self) -> anyhow::Result<Connection> {
        let connection = Connection::open(&self.path)
            .with_context(|| format!("failed to open wake gate {}", self.path.display()))?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        Ok(connection)
    }
}

fn unix_seconds() -> anyhow::Result<i64> {
    Ok(i64::try_from(
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_applies_one_limit_across_instances() {
        let root = std::env::temp_dir().join(new_id("wake-gate-test"));
        let path = root.join("gate.sqlite");
        let first = WakeGate::open(&path, 2, 60).unwrap();
        let second = WakeGate::open(&path, 2, 60).unwrap();
        let a = first.acquire("one", "ada", 1).unwrap().unwrap();
        second.acquire("two", "cass", 1).unwrap().unwrap();
        assert!(first.acquire("one", "friday", 1).unwrap().is_none());
        assert!(first.release("one", "ada", 1, &a.token).unwrap());
        assert!(second.acquire("two", "friday", 1).unwrap().is_some());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_acquisition_never_reveals_the_active_token() {
        let root = std::env::temp_dir().join(new_id("wake-gate-token-test"));
        let gate = WakeGate::open(root.join("gate.sqlite"), 2, 60).unwrap();
        let permit = gate.acquire("one", "ada", 1).unwrap().unwrap();

        assert!(gate.acquire("one", "ada", 1).unwrap().is_none());
        assert!(gate.validate("one", "ada", 1, &permit.token).unwrap());
        assert!(!gate.validate("one", "ada", 1, "wrong-token").unwrap());
        assert!(!gate.validate("one", "ada", 2, &permit.token).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
