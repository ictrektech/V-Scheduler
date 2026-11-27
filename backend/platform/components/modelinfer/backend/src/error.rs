//! 统一 HTTP 响应与错误模型（对齐 specs/docs/design/vos-http-response-spec/）
//!
//! - 响应体统一 `ResultBody { code, category, data, msg, version }`
//! - `code == 0` 成功；`code != 0` 失败（前端按 code 判成败，msg 仅调试辅助）
//! - HTTP 状态码统一 200（业务错误走 body.code）
//! - 本组件错误码区间 `[20700, +∞)`（与 204xx/205xx/206xx 区段区分）

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// 应用 category 字面值（对齐 vos-http-response-spec 应用自定义段）
pub const CATEGORY: &str = "modelinfer";

/// VOS HTTP 统一响应顶层结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultBody<T> {
    pub code: i32,
    #[serde(default = "default_category")]
    pub category: String,
    pub data: Option<T>,
    pub msg: String,
    pub version: String,
}

fn default_category() -> String {
    "ok".to_owned()
}

impl<T> ResultBody<T> {
    pub fn ok(data: T, version: impl Into<String>) -> Self {
        Self {
            code: 0,
            category: "ok".to_owned(),
            data: Some(data),
            msg: String::new(),
            version: version.into(),
        }
    }

    pub fn failed(code: i32, msg: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            code,
            category: category_for_code(code).to_owned(),
            data: None,
            msg: msg.into(),
            version: version.into(),
        }
    }
}

/// 按 code 范围回退 category（对齐 vos-http-response-spec §3.3.3）
pub fn category_for_code(code: i32) -> &'static str {
    match code {
        0 => "ok",
        1..=9999 => "system",
        10000..=19999 => "vos-platform",
        // [20000, +∞) 应用自定义区间：本组件错误码 [20700, +∞)
        _ => CATEGORY,
    }
}

/// 本组件业务错误码（category = "modelinfer"，区间 [20700, +∞)）
pub mod codes {
    /// 参数校验失败
    pub const PARAM_INVALID: i32 = 20701;
    /// 资源不存在（节点 / 配置 / 模型）
    pub const NOT_FOUND: i32 = 20702;
    /// 资源已存在（节点 host 重复等）
    pub const EXISTS: i32 = 20703;
    /// SSH 不可达 / 认证失败
    pub const SSH_CONNECT_FAILED: i32 = 20704;
    /// SSH 命令执行非零退出
    pub const SSH_COMMAND_FAILED: i32 = 20705;
    /// 平台 API 调用失败（token 缺失 / HTTP 错误）
    pub const PLATFORM_CALL_FAILED: i32 = 20706;
    /// model hub 存储空间 / NFS 导出解析失败
    pub const NFS_RESOLVE_FAILED: i32 = 20707;
    /// 节点 NFS 挂载失败
    pub const NFS_MOUNT_FAILED: i32 = 20708;
    /// 节点无对应挂载（卸载 / 状态查询）
    pub const NFS_NOT_MOUNTED: i32 = 20709;
    /// 持久化失败
    pub const DB_FAILED: i32 = 20710;
    /// 文件 IO 失败
    pub const IO_FAILED: i32 = 20711;
    /// 节点被部署配置引用，禁止删除
    pub const NODE_IN_USE: i32 = 20712;
    /// 模型广场 index.yml / manifest 拉取或解析失败
    pub const PLAZA_FETCH_FAILED: i32 = 20713;
    /// 模型包下载失败
    pub const PLAZA_DOWNLOAD_FAILED: i32 = 20714;
    /// manifest 缺必填字段 / 不一致
    pub const MANIFEST_INVALID: i32 = 20715;
    /// sha256 校验不匹配
    pub const CHECKSUM_MISMATCH: i32 = 20716;
    /// 节点硬件扫描失败（SSH 通道错误）
    pub const NODE_SCAN_FAILED: i32 = 20717;
    /// model-hub 中无匹配模型变体
    pub const MODEL_NOT_FOUND: i32 = 20718;
    /// 部署冲突（节点 hostname 重复 / 已有运行中配置）
    pub const DEPLOYMENT_CONFLICT: i32 = 20719;
    /// 部署状态不允许该操作
    pub const DEPLOYMENT_STATE_INVALID: i32 = 20720;
    /// Temporal 连接 / workflow 启动失败
    pub const TEMPORAL_FAILED: i32 = 20721;
    /// 生命周期阶段脚本失败
    pub const LIFECYCLE_STAGE_FAILED: i32 = 20722;
    /// 模型包分发 / 解压失败
    pub const MODEL_DISTRIBUTION_FAILED: i32 = 20723;
    /// 本地模型卡片包非法（格式不支持 / 未找到 model.tar.gz 或 manifest.yaml / tar 条目非法）
    pub const CARD_PACKAGE_INVALID: i32 = 20724;
    /// 模型卡片 lifecycle-order 取值非法（创建 / 更新部署配置时校验）
    pub const LIFECYCLE_ORDER_INVALID: i32 = 20725;
    /// 数据库配置非法（未提供平台 postgres / 连接串非 postgres）
    pub const DATABASE_CONFIG_INVALID: i32 = 20726;
    /// 所选节点数超出模型卡片 `caps.max-nodes` 声明的上限
    pub const CAPS_LIMIT_EXCEEDED: i32 = 20727;
    /// 卡片声明镜像在节点与 image-hub 上均不存在（中止启动）
    pub const IMAGE_NOT_AVAILABLE: i32 = 20728;
    /// 经 image-hub.sh 拉取镜像失败 / 拉取后仍缺失
    pub const IMAGE_PULL_FAILED: i32 = 20729;
    /// image-hub 地址无法确定或 registry 不可达
    pub const IMAGE_HUB_UNREACHABLE: i32 = 20730;
    /// 统一网关（V-Scheduler / 兼容旧 V-Router）不可用（未安装 / 不可达 / 无凭据 / 注册失败）
    pub const VROUTER_UNAVAILABLE: i32 = 20731;
    /// 本地反代启动 / 端口绑定失败
    pub const LOCAL_PROXY_FAILED: i32 = 20732;
    /// 节点 SSH 认证方式 / 凭据组合非法
    pub const NODE_AUTH_INVALID: i32 = 20733;
    /// 节点监控组件部署失败（容器启动 / 校验不通过）
    pub const MONITOR_DEPLOY_FAILED: i32 = 20734;
    /// consul 不可达 / 注册注销失败
    pub const CONSUL_UNAVAILABLE: i32 = 20735;
    /// prometheus 不可达 / 查询失败
    pub const PROMETHEUS_UNAVAILABLE: i32 = 20736;
    /// 监控镜像在节点与 image-hub 上均不存在（绝不直接 docker pull）
    pub const MONITOR_IMAGE_UNAVAILABLE: i32 = 20737;
    /// 经 image-hub 拉取监控镜像失败 / 拉取后仍缺失
    pub const MONITOR_PULL_FAILED: i32 = 20738;
    /// GPU 监控组件启动失败（节点缺少可用 NVIDIA 容器运行时 / `--runtime=nvidia` 与 `--gpus all` 均不可用）
    pub const GPU_RUNTIME_UNAVAILABLE: i32 = 20739;
    /// ComfyUI 权重参数非法（类型 / 文件名 / 越界路径）
    pub const COMFYUI_WEIGHT_INVALID: i32 = 20740;
    /// ComfyUI 权重链接已存在（同实例同名）
    pub const COMFYUI_WEIGHT_EXISTS: i32 = 20741;
    /// ComfyUI 权重台账记录不存在
    pub const COMFYUI_WEIGHT_NOT_FOUND: i32 = 20742;
    /// model-hub 中不存在指定模型变体 / 文件
    pub const MODEL_HUB_FILE_NOT_FOUND: i32 = 20743;
    /// ComfyUI 权重软链接创建 / 删除 IO 失败
    pub const COMFYUI_WEIGHT_IO_FAILED: i32 = 20744;
    /// 权重覆盖非法（`source` / `model_id` 不成对或取值非法）
    pub const WEIGHT_OVERRIDE_INVALID: i32 = 20745;
    /// 镜像覆盖非法（成员类型 / arch / 镜像引用 / 条数）
    pub const IMAGE_OVERRIDE_INVALID: i32 = 20746;
    /// 额外环境变量非法（键名 / 保留前缀 / 系统变量 / 与 configs 冲突 / 超限）
    pub const EXTRA_ENV_INVALID: i32 = 20747;
    /// 自动重启配置非法（连续失败阈值超出 `30..=86400` 秒）
    pub const AUTO_RESTART_CONFIG_INVALID: i32 = 20748;
    /// 普通 SSH 用户不能通过 sudo 非交互执行 root 命令
    pub const NODE_SUDO_FAILED: i32 = 20749;
    /// 权重存储空间未开启 NFS 共享或共享所在存储卷未挂载
    pub const NFS_SHARE_UNAVAILABLE: i32 = 20750;
    /// 启动所需权重 / 镜像缺失（data 携带下载清单）
    pub const START_RESOURCES_MISSING: i32 = 20751;
    /// 启动前资源检查超时
    pub const START_PREFLIGHT_TIMEOUT: i32 = 20752;
    /// 本机节点 nsenter 执行 / 上传失败（容器缺少 pid: host / privileged）
    pub const LOCAL_EXEC_FAILED: i32 = 20753;
    /// 运行配置卡片文件 override 非法（路径 / 类型 / 大小 / 条数）
    pub const FILE_OVERRIDE_INVALID: i32 = 20754;
    /// 卡片文件不存在（override 源文件失效）
    pub const CARD_FILE_NOT_FOUND: i32 = 20755;
    /// 节点被活跃运行配置占用，拒绝关机 / 重启（需显式强制）
    pub const NODE_POWER_IN_USE: i32 = 20756;
    /// 节点关机 / 重启命令下发失败
    pub const NODE_POWER_FAILED: i32 = 20757;
}

/// 本组件错误：携带 code + msg，转换为 ResultBody JSON 响应
#[derive(Debug, thiserror::Error)]
#[error("{msg}")]
pub struct ModelinferError {
    pub code: i32,
    pub msg: String,
    pub data: Option<serde_json::Value>,
}

impl ModelinferError {
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
            data: None,
        }
    }

    pub fn with_data(mut self, data: serde_json::Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// 从 trackerror `Error` 映射为本组件错误（保留 errno 与诊断信息）
impl From<trackerror::Error> for ModelinferError {
    fn from(e: trackerror::Error) -> Self {
        ModelinferError {
            code: e.as_errno(),
            msg: e.get_error_msg(),
            data: None,
        }
    }
}

impl IntoResponse for ModelinferError {
    fn into_response(self) -> Response {
        let mut body =
            ResultBody::<serde_json::Value>::failed(self.code, self.msg, env!("CARGO_PKG_VERSION"));
        body.data = self.data;
        (StatusCode::OK, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_range_fallback() {
        assert_eq!(category_for_code(0), "ok");
        assert_eq!(category_for_code(1), "system");
        assert_eq!(category_for_code(10001), "vos-platform");
        assert_eq!(category_for_code(20601), CATEGORY);
        assert_eq!(category_for_code(20701), CATEGORY);
        assert_eq!(category_for_code(99999), CATEGORY);
    }

    #[test]
    fn result_body_ok_and_failed() {
        let ok = ResultBody::ok(42i32, "0.1.0");
        assert_eq!(ok.code, 0);
        assert_eq!(ok.category, "ok");
        assert_eq!(ok.data, Some(42));

        let err = ResultBody::<serde_json::Value>::failed(20701, "bad", "0.1.0");
        assert_eq!(err.code, 20701);
        assert_eq!(err.category, CATEGORY);
        assert!(err.data.is_none());
    }
}
