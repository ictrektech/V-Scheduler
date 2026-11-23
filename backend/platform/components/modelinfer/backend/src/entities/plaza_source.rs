//! 模型广场来源实体：`modelinfer_plaza_sources`
//!
//! 一个来源 = 一个在线 HTTP 静态文件服务器基地址（见 model-plaza-server 规范）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_plaza_sources")]
pub struct Model {
    /// 来源 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 来源名称
    pub name: String,
    /// 静态服务器基地址（无尾斜杠，如 http://10.100.10.24:18088）
    pub base_url: String,
    /// 是否启用
    #[sea_orm(default_value = true)]
    pub enabled: bool,
    /// 最近一次成功拉取 index.yml 的时间（CST ISO-8601）
    pub last_synced_at: Option<String>,
    /// 最近一次拉取到的卡片数
    #[sea_orm(default_value = 0)]
    pub card_count: i32,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}