use agentic_protocol::{ChatMessage, SessionSummary, UiMessage, UiPart, UiRole};
use sea_orm::{
    ActiveValue::Set, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, FromQueryResult, QueryFilter, QueryOrder, Statement, sea_query::OnConflict,
};

use crate::entity::{message, session};

pub async fn upsert(db: &DatabaseConnection, id: &str, title: &str) -> Result<(), DbErr> {
    let result = session::Entity::insert(session::ActiveModel {
        id: Set(id.to_owned()),
        title: Set(title.to_owned()),
        created_at: Set(chrono::Utc::now().fixed_offset()),
        updated_at: Set(chrono::Utc::now().fixed_offset()),
    })
    .on_conflict(
        OnConflict::column(session::Column::Id)
            .update_columns([session::Column::UpdatedAt])
            .to_owned(),
    )
    .exec(db)
    .await;

    match result {
        Ok(_) => Ok(()),
        // SeaORM raises this even with update_columns when the resulting set is empty;
        // for an idempotent upsert this is a non-error.
        Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}

pub async fn touch_updated_at(db: &impl ConnectionTrait, id: &str) -> Result<(), DbErr> {
    session::Entity::update_many()
        .set(session::ActiveModel {
            updated_at: Set(chrono::Utc::now().fixed_offset()),
            ..Default::default()
        })
        .filter(session::Column::Id.eq(id))
        .exec(db)
        .await
        .map(|_| ())
}

pub async fn list(db: &DatabaseConnection) -> Result<Vec<SessionSummary>, DbErr> {
    #[derive(FromQueryResult)]
    struct Row {
        id: String,
        title: String,
        updated_at: chrono::DateTime<chrono::FixedOffset>,
        message_count: i64,
    }

    let rows = Row::find_by_statement(Statement::from_string(
        db.get_database_backend(),
        r#"
            SELECT s.id, s.title, s.updated_at,
                   CAST(COUNT(m.id) AS BIGINT) AS message_count
            FROM sessions s
            LEFT JOIN messages m ON m.session_id = s.id
            GROUP BY s.id, s.title, s.updated_at
            ORDER BY s.updated_at DESC
        "#
        .to_owned(),
    ))
    .all(db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SessionSummary {
            id: r.id,
            title: r.title,
            updated_at_secs: r.updated_at.timestamp() as u64,
            message_count: r.message_count as u64,
        })
        .collect())
}

pub async fn load_history(
    db: &DatabaseConnection,
    session_id: &str,
) -> Result<Vec<ChatMessage>, DbErr> {
    let messages = message::Entity::find()
        .filter(message::Column::SessionId.eq(session_id))
        .order_by_asc(message::Column::Position)
        .all(db)
        .await?;

    Ok(messages
        .iter()
        .filter_map(|m| {
            let parts: Vec<UiPart> = serde_json::from_value(m.parts.clone()).ok()?;
            let content = text_content(&parts);
            if content.is_empty() {
                return None;
            }
            Some(ChatMessage {
                role: m.role.clone(),
                content,
            })
        })
        .collect())
}

pub async fn load_ui_messages(
    db: &DatabaseConnection,
    session_id: &str,
) -> Result<Vec<UiMessage>, DbErr> {
    let messages = message::Entity::find()
        .filter(message::Column::SessionId.eq(session_id))
        .order_by_asc(message::Column::Position)
        .all(db)
        .await?;

    messages
        .iter()
        .map(|m| {
            let parts: Vec<UiPart> =
                serde_json::from_value(m.parts.clone()).unwrap_or_default();
            Ok(UiMessage {
                role: if m.role == "user" {
                    UiRole::User
                } else {
                    UiRole::Assistant
                },
                parts,
            })
        })
        .collect()
}

fn text_content(parts: &[UiPart]) -> String {
    parts
        .iter()
        .filter_map(|part| match part {
            UiPart::Text { content } => Some(content.as_str()),
            _ => None,
        })
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
