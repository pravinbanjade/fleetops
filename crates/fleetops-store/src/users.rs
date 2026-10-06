use crate::{db_err, Store};
use fleetops_core::{Error, Result, Role, User};

const COLS: &str = "id, username, password_hash, role, created_at, updated_at";

#[derive(sqlx::FromRow)]
struct UserRow {
    id: i64,
    username: String,
    password_hash: String,
    role: String,
    created_at: i64,
    updated_at: i64,
}

impl UserRow {
    fn into_parts(self) -> Result<(User, String)> {
        let role = self.role.parse::<Role>()?;
        let user = User {
            id: self.id,
            username: self.username,
            role,
            created_at: self.created_at,
            updated_at: self.updated_at,
        };
        Ok((user, self.password_hash))
    }

    fn into_user(self) -> Result<User> {
        self.into_parts().map(|(u, _)| u)
    }
}

impl Store {
    pub async fn create_user(
        &self,
        username: &str,
        password_hash: &str,
        role: Role,
        now: i64,
    ) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query_as::<_, UserRow>(&format!(
            "INSERT INTO users (username, password_hash, role, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?) RETURNING {COLS}"
        ))
        .bind(username)
        .bind(password_hash)
        .bind(role.as_str())
        .bind(now)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        row.into_user()
    }

    /// Creates the initial admin in a single conditional INSERT so that two
    /// concurrent setup requests can never both succeed.
    pub async fn create_first_admin(
        &self,
        username: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let row = sqlx::query_as::<_, UserRow>(&format!(
            "INSERT INTO users (username, password_hash, role, created_at, updated_at)
             SELECT ?, ?, 'admin', ?, ? WHERE NOT EXISTS (SELECT 1 FROM users)
             RETURNING {COLS}"
        ))
        .bind(username)
        .bind(password_hash)
        .bind(now)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(row) = row else {
            return Err(Error::Conflict("setup already completed".into()));
        };
        tx.commit().await.map_err(db_err)?;
        row.into_user()
    }

    pub async fn count_users(&self) -> Result<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)
    }

    pub async fn find_credentials(&self, username: &str) -> Result<Option<(User, String)>> {
        let row =
            sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users WHERE username = ?"))
                .bind(username)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?;
        row.map(UserRow::into_parts).transpose()
    }

    pub async fn get_user(&self, id: i64) -> Result<User> {
        let row = sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users WHERE id = ?"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        row.ok_or_else(|| Error::NotFound(format!("user {id}")))?
            .into_user()
    }

    pub async fn list_users(&self) -> Result<Vec<User>> {
        sqlx::query_as::<_, UserRow>(&format!("SELECT {COLS} FROM users ORDER BY id"))
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?
            .into_iter()
            .map(UserRow::into_user)
            .collect()
    }

    /// The last-admin guard is part of the UPDATE statement itself, so it is
    /// atomic with respect to concurrent demotions.
    pub async fn set_user_role(&self, id: i64, role: Role, now: i64) -> Result<User> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let res = sqlx::query(
            "UPDATE users SET role = ?1, updated_at = ?2
             WHERE id = ?3
               AND NOT (role = 'admin' AND ?1 <> 'admin'
                        AND (SELECT COUNT(*) FROM users WHERE role = 'admin') <= 1)",
        )
        .bind(role.as_str())
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            drop(tx);
            self.get_user(id).await?;
            return Err(Error::Conflict("cannot demote the last admin".into()));
        }
        tx.commit().await.map_err(db_err)?;
        self.get_user(id).await
    }

    pub async fn set_password_hash(&self, id: i64, password_hash: &str, now: i64) -> Result<()> {
        let res = sqlx::query("UPDATE users SET password_hash = ?, updated_at = ? WHERE id = ?")
            .bind(password_hash)
            .bind(now)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        if res.rows_affected() == 0 {
            return Err(Error::NotFound(format!("user {id}")));
        }
        Ok(())
    }

    pub async fn delete_user(&self, id: i64) -> Result<()> {
        let res = sqlx::query(
            "DELETE FROM users WHERE id = ?1
               AND NOT (role = 'admin' AND (SELECT COUNT(*) FROM users WHERE role = 'admin') <= 1)",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        if res.rows_affected() == 0 {
            self.get_user(id).await?;
            return Err(Error::Conflict("cannot delete the last admin".into()));
        }
        Ok(())
    }
}
