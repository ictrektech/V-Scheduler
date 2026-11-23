//! 脚本执行日志实体：`modelinfer_run_logs`
//!
//! 一行 = 某次运行中，某阶段脚本在某节点上的一次执行（含重试 attempt）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_run_logs")]
pub struct Model {
    /// 日志 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 运行 ID
    #[sea_orm(indexed)]
    pub run_id: String,
    /// 运行配置 ID
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 运行类型：start / stop / health
    pub kind: String,
    /// 阶段名
    #[sea_orm(default_value = "")]
    pub stage: String,
    /// 节点 ID
    #[sea_orm(default_value = "")]
    pub node_id: String,
    /// 节点名
    #[sea_orm(default_value = "")]
    pub node_name: String,
    /// 节点地址
    #[sea_orm(default_value = "")]
    pub node_host: String,
    /// 尝试次数（从 1 开始）
    #[sea_orm(default_value = 1)]
    pub attempt: i32,
    /// 退出码
    #[sea_orm(default_value = 0)]
    pub exit_code: i32,
    /// 标准输出（可能被截断）
    #[sea_orm(column_type = "Text", default_value = "")]
    pub stdout: String,
    /// 标准错误（可能被截断）
    #[sea_orm(column_type = "Text", default_value = "")]
    pub stderr: String,
    /// 原始 stdout 字节数
    #[sea_orm(default_value = 0)]
    pub stdout_bytes: i64,
    /// 原始 stderr 字节数
    #[sea_orm(default_value = 0)]
    pub stderr_bytes: i64,
    /// 输出是否被截断
    #[sea_orm(default_value = false)]
    pub truncated: bool,
    /// 开始时间（CST ISO-8601）
    pub started_at: String,
    /// 结束时间（CST ISO-8601）
    pub finished_at: String,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}