use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "tools")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    #[sea_orm(unique)]
    pub name: String,
    pub current_version_id: Option<Uuid>,
    pub owner: String,
    pub created_at: DateTimeWithTimeZone,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::tool_version::Entity")]
    Version,
}

impl Related<super::tool_version::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Version.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
