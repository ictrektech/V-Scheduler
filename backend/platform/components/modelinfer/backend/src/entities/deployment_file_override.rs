//! 运行配置级卡片文件 override：`modelinfer_deployment_file_overrides`

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_deployment_file_overrides")]
pub struct Model {
    /// override ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 所属运行配置
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 相对卡片根的脚本路径（仅 `scripts/**`）
    pub path: String,
    /// override 文本内容
    #[sea_orm(column_type = "Text", default_value = "")]
    pub content: String,
    /// 内容字节数
    #[sea_orm(default_value = 0)]
    pub size_bytes: i64,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
