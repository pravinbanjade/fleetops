use crate::{db_err, Store};
use fleetops_core::access::{EnvRole, Grant};
use fleetops_core::{Error, Result};
use std::collections::{BTreeSet, HashMap};

/// One user's grant on one environment, with the names a page shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingRecord {
    pub user_id: i64,
    pub username: String,
    pub environment_id: i64,
    pub env_name: String,
    pub env_kind: String,
    pub grant: Grant,
    pub updated_at: i64,
}

#[derive(sqlx::FromRow)]
struct Row {
    user_id: i64,
    username: String,
    environment_id: i64,
    env_name: String,
    env_kind: String,
    role: String,
    namespaces: Option<String>,
    updated_at: i64,
}

fn grant(role: &str, namespaces: Option<&str>) -> Result<Grant> {
    let namespaces = namespaces
        .map(|j| serde_json::from_str::<BTreeSet<String>>(j))
        .transpose()
        .map_err(|e| Error::Internal(format!("bad namespaces in role_bindings: {e}")))?;
    Ok(Grant {
        role: role.parse()?,
        namespaces,
    })
}

impl Row {
    fn into_record(self) -> Result<BindingRecord> {
        Ok(BindingRecord {
            grant: grant(&self.role, self.namespaces.as_deref())?,
            user_id: self.user_id,
            username: self.username,
            environment_id: self.environment_id,
            env_name: self.env_name,
            env_kind: self.env_kind,
            updated_at: self.updated_at,
        })
    }
}

const SELECT: &str = "SELECT b.user_id, u.username, b.environment_id, e.name AS env_name,
        e.kind AS env_kind, b.role, b.namespaces, b.updated_at
    FROM role_bindings b
    JOIN users u ON u.id = b.user_id
    JOIN environments e ON e.id = b.environment_id";

impl Store {
    /// The user's access to one environment, read fresh: admins have full
    /// operator access whatever their rows say.
    pub async fn env_grant(&self, user_id: i64, env_id: i64) -> Result<Option<Grant>> {
        let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT u.role, b.role, b.namespaces FROM users u
             LEFT JOIN role_bindings b ON b.user_id = u.id AND b.environment_id = ?2
             WHERE u.id = ?1",
        )
        .bind(user_id)
        .bind(env_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        match row {
            None => Ok(None),
            Some((u, _, _)) if u == "admin" => Ok(Some(Grant::full(EnvRole::Operator))),
            Some((_, Some(role), ns)) => grant(&role, ns.as_deref()).map(Some),
            Some(_) => Ok(None),
        }
    }

    /// Every environment the user can see, with their grant on it.
    pub async fn user_grants(&self, user_id: i64) -> Result<HashMap<i64, Grant>> {
        let admin: Option<String> = sqlx::query_scalar("SELECT role FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        if admin.as_deref() == Some("admin") {
            let ids: Vec<i64> = sqlx::query_scalar("SELECT id FROM environments")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            return Ok(ids
                .into_iter()
                .map(|id| (id, Grant::full(EnvRole::Operator)))
                .collect());
        }
        self.user_bindings(user_id)
            .await?
            .into_iter()
            .map(|b| Ok((b.environment_id, b.grant)))
            .collect()
    }

    pub async fn set_binding(
        &self,
        user_id: i64,
        env_id: i64,
        role: EnvRole,
        namespaces: Option<&BTreeSet<String>>,
        now: i64,
    ) -> Result<BindingRecord> {
        self.get_user(user_id).await?;
        let env = self.get_environment(env_id).await?;
        if namespaces.is_some() && !env.kind.starts_with("kube-") {
            return Err(Error::Invalid(
                "only a Kubernetes environment takes namespaces".into(),
            ));
        }
        let ns = namespaces.map(|n| serde_json::to_string(n).expect("names serialise"));
        sqlx::query(
            "INSERT INTO role_bindings (user_id, environment_id, role, namespaces, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT (user_id, environment_id)
             DO UPDATE SET role = excluded.role, namespaces = excluded.namespaces, updated_at = excluded.updated_at",
        )
        .bind(user_id)
        .bind(env_id)
        .bind(role.as_str())
        .bind(ns)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        sqlx::query_as::<_, Row>(&format!(
            "{SELECT} WHERE b.user_id = ? AND b.environment_id = ?"
        ))
        .bind(user_id)
        .bind(env_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?
        .into_record()
    }

    pub async fn delete_binding(&self, user_id: i64, env_id: i64) -> Result<bool> {
        let r = sqlx::query("DELETE FROM role_bindings WHERE user_id = ? AND environment_id = ?")
            .bind(user_id)
            .bind(env_id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(r.rows_affected() > 0)
    }

    pub async fn user_bindings(&self, user_id: i64) -> Result<Vec<BindingRecord>> {
        sqlx::query_as::<_, Row>(&format!("{SELECT} WHERE b.user_id = ? ORDER BY e.id"))
            .bind(user_id)
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
            .into_iter()
            .map(Row::into_record)
            .collect()
    }

    pub async fn env_bindings(&self, env_id: i64) -> Result<Vec<BindingRecord>> {
        sqlx::query_as::<_, Row>(&format!(
            "{SELECT} WHERE b.environment_id = ? ORDER BY u.username COLLATE NOCASE"
        ))
        .bind(env_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(Row::into_record)
        .collect()
    }
}
