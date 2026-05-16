pub mod entity;
pub mod repo;

pub use sea_orm::DatabaseConnection;

use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DbErr, Schema,
    sea_query::TableCreateStatement,
};

use entity::{message, session};

pub async fn connect(url: &str) -> Result<DatabaseConnection, DbErr> {
    let mut opt = ConnectOptions::new(url);
    opt.max_connections(10).sqlx_logging(false);
    let db = Database::connect(opt).await?;
    sync_schema(&db).await?;
    Ok(db)
}

async fn sync_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    let backend = db.get_database_backend();
    let schema = Schema::new(backend);

    let stmts: Vec<TableCreateStatement> = vec![
        schema.create_table_from_entity(session::Entity).if_not_exists().to_owned(),
        schema.create_table_from_entity(message::Entity).if_not_exists().to_owned(),
    ];

    for stmt in stmts {
        db.execute(&stmt).await?;
    }

    // Index for efficient per-session message loading
    if backend == DatabaseBackend::Postgres {
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id, position)",
        )
        .await?;
    }

    Ok(())
}
