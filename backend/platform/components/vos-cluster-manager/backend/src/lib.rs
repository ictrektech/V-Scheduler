//! vos-cluster-manager 组件库入口
//!
//! 负责 VOS 集群管理：集群创建 / 成员增删的编排入口（用户输入节点 IP + 管理员密码，
//! 持久化到 etcd 集群状态）、以及基于 VOS 平台 projects 原语（`/v1000/app/projects/*`）
//! 动态启停 etcd、postgres-ha（patroni）子项目。
//!
//! 阶段定位：当前为组件骨架（foundation），提供前后端一体 HTTP 服务 + VOS 应用打包；
//! 集群编排能力按 `docs/roadmap.md` 分阶段落地。

pub mod config;
pub mod db;
pub mod entity;
pub mod error;
pub mod etcd;
pub mod handlers;
pub mod logging;
pub mod orchestration;
pub mod projects;
pub mod remote;
pub mod router;
pub mod schema;
pub mod state;
pub mod temporal;

pub use config::Config;
pub use router::build_router;
pub use state::AppState;