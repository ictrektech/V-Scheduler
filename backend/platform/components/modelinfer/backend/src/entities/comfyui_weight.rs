//! ComfyUI 实例权重台账实体：`modelinfer_comfyui_weights`
//!
//! 一行 = 把 model-hub 某个变体中的一个文件，以某个类型软链进某运行配置实例权重目录的记录。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_comfyui_weights")]
pub struct Model {
    /// 记录 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 运行配置 ID（实例标识）
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 来源：hf / ms
    pub source: String,
    /// model-hub 权重 id（`<org>/<repo>`）
    pub weight_id: String,
    /// 变体内文件相对路径（如 `checkpoints/a.safetensors`）
    pub file_path: String,
    /// 目标类型目录（预设或自定义相对子目录）
    pub target_type: String,
    /// 链接文件名
    pub link_name: String,
    /// 实例权重目录内相对路径（`<target_type>/<link_name>`）
    pub link_rel: String,
    /// 软链接目标（相对路径）
    pub target_rel: String,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}