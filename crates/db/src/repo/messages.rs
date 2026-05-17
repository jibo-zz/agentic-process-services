use agentic_protocol::{UiMessage, UiRole};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, PaginatorTrait, QueryFilter,
};
use uuid::Uuid;

use crate::entity::message;

pub async fn count(db: &DatabaseConnection, session_id: &str) -> Result<i32, DbErr> {
    let n = message::Entity::find()
        .filter(message::Column::SessionId.eq(session_id))
        .count(db)
        .await?;
    Ok(n as i32)
}

pub async fn insert(
    db: &impl ConnectionTrait,
    session_id: &str,
    position: i32,
    ui_message: &UiMessage,
) -> Result<(), DbErr> {
    let role = match ui_message.role {
        UiRole::User => "user",
        UiRole::Assistant => "assistant",
    };
    let parts =
        serde_json::to_value(&ui_message.parts).map_err(|e| DbErr::Custom(e.to_string()))?;

    message::ActiveModel {
        id: Set(Uuid::new_v4()),
        session_id: Set(session_id.to_owned()),
        role: Set(role.to_owned()),
        position: Set(position),
        parts: Set(parts),
        created_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db)
    .await
    .map(|_| ())
}
