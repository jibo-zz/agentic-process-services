use agentic_protocol::{
    SaveDraftParams, ToolRow, ToolScriptLanguage, ToolTestCase, ToolVersionRow, ToolVersionStatus,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ColumnTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, QueryOrder, TransactionTrait,
};
use uuid::Uuid;

use crate::entity::{tool, tool_version};

const STATUS_DRAFT: &str = "draft";
const STATUS_ACTIVE: &str = "active";
const STATUS_DEPRECATED: &str = "deprecated";
const LANG_PYTHON: &str = "python";
const LANG_SHELL: &str = "shell";

/// Returns active tool versions joined with their parent tool. Used by `tools.list` to
/// surface DB-authored tools alongside the static registry.
pub async fn list_active(
    db: &DatabaseConnection,
) -> Result<Vec<(tool::Model, tool_version::Model)>, DbErr> {
    let tools = tool::Entity::find()
        .filter(tool::Column::CurrentVersionId.is_not_null())
        .all(db)
        .await?;

    let mut pairs = Vec::with_capacity(tools.len());
    for t in tools {
        let Some(version_id) = t.current_version_id else {
            continue;
        };
        if let Some(v) = tool_version::Entity::find_by_id(version_id).one(db).await? {
            pairs.push((t, v));
        }
    }
    Ok(pairs)
}

/// Fetches the currently active version of a named tool, used when dispatching a Tier-2 call.
pub async fn get_active_version(
    db: &DatabaseConnection,
    name: &str,
) -> Result<Option<tool_version::Model>, DbErr> {
    let Some(t) = tool::Entity::find()
        .filter(tool::Column::Name.eq(name))
        .one(db)
        .await?
    else {
        return Ok(None);
    };
    let Some(version_id) = t.current_version_id else {
        return Ok(None);
    };
    tool_version::Entity::find_by_id(version_id).one(db).await
}

/// Returns every tool with its full version history. Drives the management UI.
pub async fn management(db: &DatabaseConnection) -> Result<Vec<ToolRow>, DbErr> {
    let tools = tool::Entity::find()
        .order_by_desc(tool::Column::UpdatedAt)
        .all(db)
        .await?;

    let mut rows = Vec::with_capacity(tools.len());
    for t in tools {
        let versions = tool_version::Entity::find()
            .filter(tool_version::Column::ToolId.eq(t.id))
            .order_by_desc(tool_version::Column::Version)
            .all(db)
            .await?;
        rows.push(to_tool_row(&t, &versions));
    }
    Ok(rows)
}

/// Saves a new draft version. Creates the parent `tool` row if it does not exist;
/// version numbers are monotonic per tool.
pub async fn save_draft(
    db: &DatabaseConnection,
    params: SaveDraftParams,
) -> Result<ToolVersionRow, DbErr> {
    let txn = db.begin().await?;
    let now = chrono::Utc::now().fixed_offset();

    let parent = match tool::Entity::find()
        .filter(tool::Column::Name.eq(&params.name))
        .one(&txn)
        .await?
    {
        Some(existing) => existing,
        None => {
            let new_tool = tool::ActiveModel {
                id: Set(Uuid::new_v4()),
                name: Set(params.name.clone()),
                current_version_id: Set(None),
                owner: Set(params.owner.clone()),
                created_at: Set(now),
                updated_at: Set(now),
            };
            new_tool.insert(&txn).await?
        }
    };

    let next_version = tool_version::Entity::find()
        .filter(tool_version::Column::ToolId.eq(parent.id))
        .order_by_desc(tool_version::Column::Version)
        .one(&txn)
        .await?
        .map(|v| v.version + 1)
        .unwrap_or(1);

    let version_id = Uuid::new_v4();
    let tests_json = serde_json::to_value(&params.tests).unwrap_or(serde_json::json!([]));
    let active_version = tool_version::ActiveModel {
        id: Set(version_id),
        tool_id: Set(parent.id),
        version: Set(next_version),
        language: Set(language_to_str(params.language).to_owned()),
        script: Set(params.script.clone()),
        args_schema: Set(params.args_schema.clone()),
        output_schema: Set(params.output_schema.clone()),
        tests: Set(tests_json.clone()),
        status: Set(STATUS_DRAFT.to_owned()),
        risk: Set(risk_to_str(&params.risk).to_owned()),
        timeout_ms: Set(params.timeout_ms),
        description: Set(params.description.clone()),
        created_at: Set(now),
    };
    let saved = active_version.insert(&txn).await?;

    // Bump the tool's updated_at so management list sorts newest-first.
    tool::Entity::update_many()
        .col_expr(tool::Column::UpdatedAt, now.into())
        .filter(tool::Column::Id.eq(parent.id))
        .exec(&txn)
        .await?;

    txn.commit().await?;
    Ok(to_version_row(&saved))
}

/// Promotes a draft version to `active`. Demotes the previously active version of the same
/// tool (if any) to `deprecated` and points `tools.current_version_id` at the new one.
pub async fn register_active(
    db: &DatabaseConnection,
    version_id: Uuid,
) -> Result<ToolVersionRow, DbErr> {
    let txn = db.begin().await?;
    let Some(target) = tool_version::Entity::find_by_id(version_id)
        .one(&txn)
        .await?
    else {
        return Err(DbErr::RecordNotFound(format!(
            "tool version {version_id} not found"
        )));
    };

    // Demote any currently active version of the same tool.
    tool_version::Entity::update_many()
        .col_expr(
            tool_version::Column::Status,
            STATUS_DEPRECATED.to_owned().into(),
        )
        .filter(tool_version::Column::ToolId.eq(target.tool_id))
        .filter(tool_version::Column::Status.eq(STATUS_ACTIVE))
        .exec(&txn)
        .await?;

    // Promote the target to active.
    let mut active: tool_version::ActiveModel = target.clone().into();
    active.status = Set(STATUS_ACTIVE.to_owned());
    let promoted = active.update(&txn).await?;

    let now = chrono::Utc::now().fixed_offset();
    tool::Entity::update_many()
        .col_expr(tool::Column::CurrentVersionId, Some(promoted.id).into())
        .col_expr(tool::Column::UpdatedAt, now.into())
        .filter(tool::Column::Id.eq(promoted.tool_id))
        .exec(&txn)
        .await?;

    txn.commit().await?;
    Ok(to_version_row(&promoted))
}

/// Deletes a tool and every version it has ever had, atomically. Returns false if the tool id
/// did not match any row. Use this for "remove this tool entirely" — for surgical version
/// removal that preserves history, use `delete_version`.
pub async fn delete_tool(db: &DatabaseConnection, tool_id: Uuid) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    let Some(_existing) = tool::Entity::find_by_id(tool_id).one(&txn).await? else {
        return Ok(false);
    };
    tool::Entity::update_many()
        .col_expr(tool::Column::CurrentVersionId, None::<Uuid>.into())
        .filter(tool::Column::Id.eq(tool_id))
        .exec(&txn)
        .await?;
    tool_version::Entity::delete_many()
        .filter(tool_version::Column::ToolId.eq(tool_id))
        .exec(&txn)
        .await?;
    tool::Entity::delete_by_id(tool_id).exec(&txn).await?;
    txn.commit().await?;
    Ok(true)
}

/// Deletes a non-active version. Refuses to delete the version currently pointed at by the
/// parent tool. The parent row is removed if no versions remain.
pub async fn delete_version(db: &DatabaseConnection, version_id: Uuid) -> Result<bool, DbErr> {
    let txn = db.begin().await?;
    let Some(version) = tool_version::Entity::find_by_id(version_id)
        .one(&txn)
        .await?
    else {
        return Ok(false);
    };

    let parent_tool_id = version.tool_id;
    let parent = tool::Entity::find_by_id(parent_tool_id).one(&txn).await?;
    if let Some(parent) = &parent
        && parent.current_version_id == Some(version_id)
    {
        return Err(DbErr::Custom(
            "cannot delete the currently active version".to_owned(),
        ));
    }

    tool_version::Entity::delete_by_id(version_id)
        .exec(&txn)
        .await?;

    // If this was the parent's last version, drop the parent too.
    let remaining = tool_version::Entity::find()
        .filter(tool_version::Column::ToolId.eq(parent_tool_id))
        .one(&txn)
        .await?;
    if remaining.is_none() {
        tool::Entity::delete_by_id(parent_tool_id)
            .exec(&txn)
            .await?;
    }

    txn.commit().await?;
    Ok(true)
}

pub fn to_tool_row(tool: &tool::Model, versions: &[tool_version::Model]) -> ToolRow {
    ToolRow {
        id: tool.id.to_string(),
        name: tool.name.clone(),
        current_version_id: tool.current_version_id.map(|id| id.to_string()),
        owner: tool.owner.clone(),
        updated_at_secs: tool.updated_at.timestamp() as u64,
        versions: versions.iter().map(to_version_row).collect(),
    }
}

pub fn to_version_row(version: &tool_version::Model) -> ToolVersionRow {
    let tests: Vec<ToolTestCase> =
        serde_json::from_value(version.tests.clone()).unwrap_or_default();
    ToolVersionRow {
        id: version.id.to_string(),
        tool_id: version.tool_id.to_string(),
        version: version.version,
        language: language_from_str(&version.language),
        script: version.script.clone(),
        args_schema: version.args_schema.clone(),
        output_schema: version.output_schema.clone(),
        tests,
        status: status_from_str(&version.status),
        risk: risk_from_str(&version.risk),
        timeout_ms: version.timeout_ms,
        description: version.description.clone(),
        created_at_secs: version.created_at.timestamp() as u64,
    }
}

pub fn language_to_str(lang: ToolScriptLanguage) -> &'static str {
    match lang {
        ToolScriptLanguage::Python => LANG_PYTHON,
        ToolScriptLanguage::Shell => LANG_SHELL,
    }
}

pub fn language_from_str(value: &str) -> ToolScriptLanguage {
    match value {
        LANG_SHELL => ToolScriptLanguage::Shell,
        _ => ToolScriptLanguage::Python,
    }
}

pub fn status_from_str(value: &str) -> ToolVersionStatus {
    match value {
        STATUS_ACTIVE => ToolVersionStatus::Active,
        STATUS_DEPRECATED => ToolVersionStatus::Deprecated,
        _ => ToolVersionStatus::Draft,
    }
}

pub fn risk_to_str(risk: &agentic_protocol::ToolRisk) -> &'static str {
    use agentic_protocol::ToolRisk::*;
    match risk {
        ReadOnly => "read_only",
        WritesFiles => "writes_files",
        DeletesFiles => "deletes_files",
        DeletesDirectories => "deletes_directories",
        Network => "network",
        ExternalProcess => "external_process",
    }
}

pub fn risk_from_str(value: &str) -> agentic_protocol::ToolRisk {
    use agentic_protocol::ToolRisk::*;
    match value {
        "read_only" => ReadOnly,
        "writes_files" => WritesFiles,
        "deletes_files" => DeletesFiles,
        "deletes_directories" => DeletesDirectories,
        "network" => Network,
        _ => ExternalProcess,
    }
}
