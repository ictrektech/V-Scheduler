//! modelinfer 组件库入口
//!
//! VOS AI 模型下载管理与集群调度应用：提供模型广场、节点管理、NFS 挂载与运行配置的
//! 管理面（Rust axum 后端 + Vue3 前端）。应用 id `com.ictrek.modelinfer`。

pub mod config;
pub mod db;
pub mod comfyui;
pub mod deploy;
pub mod deployments;
pub mod entities;
pub mod error;
pub mod handlers;
pub mod images;
pub mod inference;
pub mod localproxy;
pub mod logging;
pub mod model_family;
pub mod model_hub;
pub mod monitor;
pub mod nfs;
pub mod nodes;
pub mod observability;
pub mod paging;
pub mod platform;
pub mod plaza;
pub mod router;
pub mod ssh;
pub mod state;
pub mod time;
pub mod access;
pub mod vrouter;

pub use config::Config;
pub use router::build_router;
pub use state::AppState;
