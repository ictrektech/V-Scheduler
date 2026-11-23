//! 模型广场已登记模型实体：`modelinfer_plaza_models`
//!
//! 一行 = 一个已下载并登记的模型卡片（来自某个来源）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_plaza_models")]
pub struct Model {
    /// 主键：`<card_id>@<version>`
    #[sea_orm(primary_key, column_type = "String(StringLen::N(256))")]
    pub id: String,
    /// 模型卡片 id（manifest `id`，逆序 DNS）
    pub model_id: String,
    /// 权重 id（manifest `model`）
    #[sea_orm(default_value = "")]
    pub model: String,
    /// 名称
    pub name: String,
    /// 版本
    pub version: String,
    /// 来源：`plaza`（来自模型广场来源）
    #[sea_orm(default_value = "plaza")]
    pub source: String,
    /// 来源 id（modelinfer_plaza_sources.id；空表示手动/未知）
    #[sea_orm(default_value = "")]
    pub source_id: String,
    /// 本地存放路径（解压后的卡片目录）
    pub path: String,
    /// 本地 model.tar.gz 路径
    #[sea_orm(default_value = "")]
    pub package_path: String,
    /// model.tar.gz 的 sha256（十六进制；未知为空）
    #[sea_orm(default_value = "")]
    pub package_sha256: String,
    /// manifest.yaml 原文（JSON 字符串，解析后）
    #[sea_orm(default_value = "")]
    pub manifest_json: String,
    /// 状态：downloaded / available / error
    #[sea_orm(default_value = "available")]
    pub state: String,
    /// 大小（字节）
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