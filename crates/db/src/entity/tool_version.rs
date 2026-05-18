use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "tool_versions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tool_id: Uuid,
    pub version: i32,
    pub language: String,
    pub script: String,
    #[sea_orm(column_type = "JsonBinary")]
    pub args_schema: Json,
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub output_schema: Option<Json>,
    #[sea_orm(column_type = "JsonBinary")]
    pub tests: Json,
    pub status: String,
    pub risk: String,
    pub timeout_ms: i32,
    pub description: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::tool::Entity",
        from = "Column::ToolId",
        to = "super::tool::Column::Id",
        on_delete = "Cascade"
    )]
    Tool,
}

impl Related<super::tool::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tool.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
