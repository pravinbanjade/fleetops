use crate::{
    auth::{extract::AdminUser, password},
    routes::audit,
    ApiResult, AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use dockerer_core::{now_unix, validate_password, validate_username, Error, Role, User};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct CreateUser {
    pub username: String,
    pub password: String,
    pub role: Role,
}

#[derive(Deserialize)]
pub struct UpdateUser {
    pub role: Option<Role>,
    pub password: Option<String>,
}

pub async fn list(State(s): State<AppState>, _admin: AdminUser) -> ApiResult<Json<Vec<User>>> {
    Ok(Json(s.store.list_users().await?))
}

pub async fn create(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Json(body): Json<CreateUser>,
) -> ApiResult<(StatusCode, Json<User>)> {
    validate_username(&body.username)?;
    validate_password(&body.password)?;
    let hash = password::hash_password(body.password).await?;
    let user = s
        .store
        .create_user(&body.username, &hash, body.role, now_unix())
        .await
        .map_err(|e| match e {
            Error::Conflict(_) => {
                Error::Conflict(format!("username '{}' is already taken", body.username))
            }
            other => other,
        })?;
    audit(
        &s,
        Some(admin.user.id),
        "user.create",
        "user",
        &user.id.to_string(),
        "success",
    )
    .await;
    Ok((StatusCode::CREATED, Json(user)))
}

pub async fn update(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
    Json(body): Json<UpdateUser>,
) -> ApiResult<Json<User>> {
    if body.role.is_none() && body.password.is_none() {
        return Err(Error::Invalid("nothing to update".into()).into());
    }
    if let Some(pw) = &body.password {
        validate_password(pw)?;
    }
    s.store.get_user(id).await?;
    let now = now_unix();
    if let Some(role) = body.role {
        s.store.set_user_role(id, role, now).await?;
    }
    if let Some(pw) = body.password {
        let hash = password::hash_password(pw).await?;
        s.store.set_password_hash(id, &hash, now).await?;
        let keep = (id == admin.user.id).then_some(admin.session.id);
        s.store.delete_user_sessions(id, keep).await?;
    }
    audit(
        &s,
        Some(admin.user.id),
        "user.update",
        "user",
        &id.to_string(),
        "success",
    )
    .await;
    Ok(Json(s.store.get_user(id).await?))
}

pub async fn remove(
    State(s): State<AppState>,
    AdminUser(admin): AdminUser,
    Path(id): Path<i64>,
) -> ApiResult<StatusCode> {
    s.store.delete_user(id).await?;
    audit(
        &s,
        Some(admin.user.id),
        "user.delete",
        "user",
        &id.to_string(),
        "success",
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}
