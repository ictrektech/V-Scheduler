# com.ictrek.modelinfer 权威 Dockerfile（build context = 仓库根）
#
# 多阶段构建：
#   Stage 1 builder（vos-builder：Rust stable + node + pnpm）
#     - 构建前端 dist（Vite base=./ + hash 路由）
#     - 构建 modelinfer Rust 二进制（lib+bin，path 依赖 shared/trackerror）
#   Stage 2 runtime（ubuntu:22.04 + tini/curl/libssl3）
#
# SSH 客户端能力由 ssh2（libssh2 静态内联编译）+ 系统 libssl3 提供，
# 无需容器内 ssh 可执行文件（NFS 挂载命令在远端节点执行）。
#
# 用法（build context 必须为仓库根；-f 相对**当前工作目录**解析，勿混用）：
#   # 从仓库根执行
#   docker buildx build -f apps/com.ictrek.modelinfer/Dockerfile \
#     -t registry.ictrek.internal/ictrek/modelinfer:<ver> .
#   # 从应用目录执行（Makefile build-image 即此方式）
#   cd apps/com.ictrek.modelinfer
#   docker buildx build -f Dockerfile \
#     -t registry.ictrek.internal/ictrek/modelinfer:<ver> ../..

ARG VOS_BUILDER_IMAGE=registry.ictrek.internal/ictrek/vos-builder:latest
ARG RUNTIME_IMAGE=registry.ictrek.internal/library/ubuntu:22.04

# ============================================================
# Stage 1: Builder
# ============================================================
FROM ${VOS_BUILDER_IMAGE} AS builder

WORKDIR /build
SHELL ["/bin/bash", "-c"]
ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=Asia/Shanghai
ENV PROTOC=/usr/bin/protoc
ENV PROTOC_INCLUDE=/usr/include

# 依赖目录（path 依赖 trackerror / third_party/sea-schema）
COPY components/modelinfer/ /build/components/modelinfer/
COPY shared/trackerror/ /build/shared/trackerror/
COPY third_party/sea-schema/ /build/third_party/sea-schema/

# ---- 前端 dist ----
RUN --mount=type=cache,target=/root/.local/share/pnpm/store,sharing=locked \
    set -eux; \
    cd /build/components/modelinfer/frontend; \
    pnpm install --frozen-lockfile; \
    pnpm build; \
    test -f dist/index.html

# ---- modelinfer 二进制 ----
RUN --mount=type=cache,target=/root/.cargo/registry/,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git/db,sharing=locked \
    set -eux; \
    cd /build/components/modelinfer/backend; \
    export CARGO_INCREMENTAL=0; \
    cargo build --release --locked; \
    strip -s target/release/modelinfer || true; \
    ls -lh target/release/modelinfer

# ============================================================
# Stage 2: Runtime
# ============================================================
FROM ${RUNTIME_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=Asia/Shanghai

# APT 国内镜像 + 运行时依赖
RUN set -eux; \
    arch=$(uname -m); \
    if [ "$arch" = "x86_64" ]; then \
        sed -i 's|http://archive.ubuntu.com/ubuntu/|http://mirrors.ustc.edu.cn/ubuntu/|g' /etc/apt/sources.list && \
        sed -i 's|http://security.ubuntu.com/ubuntu/|http://mirrors.ustc.edu.cn/ubuntu/|g' /etc/apt/sources.list; \
    elif [ "$arch" = "aarch64" ] || [ "$arch" = "arm64" ]; then \
        sed -i 's|http://ports.ubuntu.com/ubuntu-ports|http://mirrors.ustc.edu.cn/ubuntu-ports|g' /etc/apt/sources.list; \
    fi; \
    apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates tini curl libssl3 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/components/modelinfer/backend/target/release/modelinfer /usr/local/bin/
COPY --from=builder /build/components/modelinfer/frontend/dist /app/dist

RUN set -eux; \
    chmod +x /usr/local/bin/modelinfer; \
    mkdir -p /var/lib/modelinfer; \
    /usr/local/bin/modelinfer --version || true; \
    test -f /app/dist/index.html

EXPOSE 19908

HEALTHCHECK --interval=30s --timeout=10s --start-period=10s --retries=3 \
    CMD curl -f "http://localhost:${VOS_MODELINFER_PORT:-19908}/app/com.ictrek.modelinfer/api/health" || exit 1

ARG APP_VERSION=dev
ARG VCS_REF=unknown
LABEL org.opencontainers.image.title="modelinfer" \
      org.opencontainers.image.version="${APP_VERSION}" \
      org.opencontainers.image.revision="${VCS_REF}"

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/modelinfer"]
CMD ["--port", "19908", "--frontend-dir", "/app/dist", "--data-dir", "/var/lib/modelinfer", "--url-base", "/app/com.ictrek.modelinfer/", "--log-level", "info"]
