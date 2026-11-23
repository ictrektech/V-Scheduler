//! image-hub：VOS 离线镜像中转组件
//!
//! - 数据面：官方 registry（Docker Distribution），HTTP 无认证，端口 19913
//! - 管理面：本服务（Rust axum 后端 + Vue 前端），端口 19914
//! - 能力：镜像查询 / 删除、远程拉取与同步（skopeo 编排）、本地克隆与改名、
//!   归档导入（docker-archive / OCI）、上游认证与代理设置、
//!   异步任务进度、registry 地址展示（网卡下拉）、上传/下载命令生成、
//!   一键客户端脚本分发（`GET <url-base>/image-hub.sh`）

pub mod config;
pub mod error;
pub mod gc;
pub mod handlers;
pub mod jobs;
pub mod logging;
pub mod mirror;
pub mod platform;
pub mod progress;
pub mod registry;
pub mod router;
pub mod scheduler;
pub mod settings;
pub mod skopeo;
pub mod state;

pub use config::Config;
pub use error::{ImageHubError, ResultBody};
pub use router::build_router;
pub use state::AppState;