# com.ictrek.image-hub 权威 Dockerfile（build context = 仓库根）
#
# 多阶段构建：
#   Stage 1 skopeo-builder（vos-builder：Go + gcc + pkg-config）
#     - 源码构建 skopeo（CGO_ENABLED=1 动态链接；DISABLE_DOCS=1 跳过 man 文档）
#   Stage 2 registry-builder（vos-builder：curl + tar）
#     - 下载官方 registry 发行归档（registry_<ver>_linux_<arch>.tar.gz）并校验 sha256，
#       解出静态 registry 二进制（单容器架构下由 entrypoint 启动 registry serve）
#   Stage 3 builder（vos-builder：Rust stable + node + pnpm）
#     - 构建前端 dist（Vite base=./ + hash 路由）
#     - 构建 image-hub Rust 二进制（lib+bin，path 依赖 shared/trackerror）
#   Stage 4 runtime（ubuntu:22.04 + tini/curl/ca-certificates/libssl3 + skopeo 运行期库）
#
# 单容器架构（见 components/image-hub/openspec/changes/image-hub-registry-gc/ D-IH-59）：
#   本镜像同时承载 registry 数据面（:19913）与管理面（:19914），由 entrypoint 在同容器内拉起，
#   不再部署独立的 registry 容器；registry 存储目录即管理面 GC 的操作目录。
#
# 用法（build context 必须为仓库根；-f 相对**当前工作目录**解析，勿混用）：
#   # 从仓库根执行
#   docker buildx build -f apps/com.ictrek.image-hub/Dockerfile \
#     -t registry.ictrek.internal/ictrek/image-hub:<ver> .
#   # 从应用目录执行（Makefile build-image 即此方式）
#   cd apps/com.ictrek.image-hub
#   docker buildx build -f Dockerfile \
#     -t registry.ictrek.internal/ictrek/image-hub:<ver> ../..

ARG VOS_BUILDER_IMAGE=registry.ictrek.internal/ictrek/vos-builder:latest
ARG RUNTIME_IMAGE=registry.ictrek.internal/library/ubuntu:22.04

# ============================================================
# Stage 1: skopeo Builder（源码构建，用于设备侧镜像下载 / 跨 registry 迁移）
# ============================================================
# 说明：
#   - Ubuntu 22.04 仓库自带 skopeo 版本较老（1.4.x），故基于源码构建 1.24.1；
#   - 源码包在构建期下载（不进 build context / 仓库），并做 sha256 校验；
#   - CGO_ENABLED=1 动态链接系统库（libgpgme / libsqlite3 等），运行期需对应库；
#   - DISABLE_DOCS=1 跳过 man 文档生成（无需 go-md2man）。
#   外部依据：https://github.com/podman-container-tools/skopeo/blob/main/install.md
FROM ${VOS_BUILDER_IMAGE} AS skopeo-builder

SHELL ["/bin/bash", "-c"]
ENV DEBIAN_FRONTEND=noninteractive

# 版本 / 源 / sha256 三者需同步变更（升级版本时一并更新）
ARG SKOPEO_VERSION=1.24.1
ARG SKOPEO_SOURCE_URL=http://files.ictrek.internal/resource/sources/skopeo-${SKOPEO_VERSION}.tar.gz
ARG SKOPEO_SOURCE_SHA256=6a3f342a327ec7684198eb0e15114940b6101506db48d9a96590f5942cb3b335

# 源码构建依赖（install.md「Building without a container」Debian/Ubuntu 清单）
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      libgpgme-dev libassuan-dev libbtrfs-dev libsqlite3-dev pkg-config; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/skopeo-build

# 下载 + 校验 + 解压（curl 为 vos-builder 内置）
RUN set -eux; \
    curl -fsSL -o skopeo.tar.gz "${SKOPEO_SOURCE_URL}"; \
    echo "${SKOPEO_SOURCE_SHA256}  skopeo.tar.gz" | sha256sum -c -; \
    tar xzf skopeo.tar.gz; \
    rm -f skopeo.tar.gz

# 构建二进制：CGO_ENABLED=1（动态链接系统库，避免编译错误）+ DISABLE_DOCS=1
RUN set -eux; \
    cd "skopeo-${SKOPEO_VERSION}"; \
    DISABLE_DOCS=1 CGO_ENABLED=1 make bin/skopeo; \
    strip -s bin/skopeo; \
    ls -lh bin/skopeo; \
    mkdir -p /out; \
    install -m 0755 bin/skopeo /out/skopeo; \
    install -m 0644 default-policy.json /out/containers-policy.json; \
    install -m 0644 default.yaml /out/registries.d-default.yaml; \
    /out/skopeo --version

# ============================================================
# Stage 2: registry Builder（下载官方 registry 发行归档，供单容器内启动 registry serve）
# ============================================================
# 说明：
#   - 归档为官方发行包（LICENSE / README.md / registry 二进制），非 docker-archive；
#   - 按构建架构选择 amd64 / arm64 资产并做 sha256 校验（与 Makefile 的 REGISTRY_IMAGE_VERSION 同步）；
#   - 二进制为静态链接，可直接用于 ubuntu 运行镜像。
FROM ${VOS_BUILDER_IMAGE} AS registry-builder

SHELL ["/bin/bash", "-c"]
ENV DEBIAN_FRONTEND=noninteractive

ARG REGISTRY_VERSION=3.1.1
ARG REGISTRY_SOURCE_URL_BASE=http://files.ictrek.internal/resource/asserts
ARG REGISTRY_SHA256_AMD64=6f330a3ba9ea1d23a6ee189f449d792595240585bb2f159123d76ac594f70dd8
ARG REGISTRY_SHA256_ARM64=8167316d2b4a57e10d44f8c8a3c75fea5f3ec1c71872760bb903e5e8e52e9ad6

RUN set -eux; \
    arch="$(uname -m)"; \
    case "${arch}" in \
      x86_64) goarch=amd64; sha="${REGISTRY_SHA256_AMD64}" ;; \
      aarch64|arm64) goarch=arm64; sha="${REGISTRY_SHA256_ARM64}" ;; \
      *) echo "不支持的构建架构: ${arch}" >&2; exit 1 ;; \
    esac; \
    url="${REGISTRY_SOURCE_URL_BASE}/registry_${REGISTRY_VERSION}_linux_${goarch}.tar.gz"; \
    curl -fsSL -o /tmp/registry.tar.gz "${url}"; \
    echo "${sha}  /tmp/registry.tar.gz" | sha256sum -c -; \
    mkdir -p /tmp/registry-extract; \
    tar xzf /tmp/registry.tar.gz -C /tmp/registry-extract; \
    test -f /tmp/registry-extract/registry; \
    mkdir -p /out; \
    install -m 0755 /tmp/registry-extract/registry /out/registry; \
    /out/registry --version

# ============================================================
# Stage 3: image-hub Builder
# ============================================================
FROM ${VOS_BUILDER_IMAGE} AS builder

WORKDIR /build
SHELL ["/bin/bash", "-c"]
ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=Asia/Shanghai

# 依赖目录（path 依赖 trackerror）
COPY components/image-hub/ /build/components/image-hub/
COPY shared/trackerror/ /build/shared/trackerror/

# ---- 前端 dist ----
RUN --mount=type=cache,target=/root/.local/share/pnpm/store,sharing=locked \
    set -eux; \
    cd /build/components/image-hub/frontend; \
    pnpm install --frozen-lockfile; \
    pnpm build; \
    test -f dist/index.html

# ---- image-hub 二进制 ----
RUN --mount=type=cache,target=/root/.cargo/registry/,sharing=locked \
    --mount=type=cache,target=/root/.cargo/git/db,sharing=locked \
    set -eux; \
    cd /build/components/image-hub/backend; \
    export CARGO_INCREMENTAL=0; \
    cargo build --release --locked; \
    strip -s target/release/image-hub || true; \
    ls -lh target/release/image-hub

# ============================================================
# Stage 4: Runtime
# ============================================================
FROM ${RUNTIME_IMAGE}

ENV DEBIAN_FRONTEND=noninteractive
ENV TZ=Asia/Shanghai

# APT 国内镜像 + 运行时依赖
#   - libgpgme11 / libsqlite3-0：skopeo（CGO 动态链接）运行期库
#     （libassuan0 / libgpg-error0 为 libgpgme11 传递依赖，由 apt 自动带入）
RUN set -eux; \
    arch=$(uname -m); \
    if [ "$arch" = "x86_64" ]; then \
        sed -i 's|http://archive.ubuntu.com/ubuntu/|http://mirrors.ustc.edu.cn/ubuntu/|g' /etc/apt/sources.list && \
        sed -i 's|http://security.ubuntu.com/ubuntu/|http://mirrors.ustc.edu.cn/ubuntu/|g' /etc/apt/sources.list; \
    elif [ "$arch" = "aarch64" ] || [ "$arch" = "arm64" ]; then \
        sed -i 's|http://ports.ubuntu.com/ubuntu-ports|http://mirrors.ustc.edu.cn/ubuntu-ports|g' /etc/apt/sources.list; \
    fi; \
    apt-get update && \
    apt-get install -y --no-install-recommends \
      ca-certificates tini curl libssl3 libgpgme11 libsqlite3-0 && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/components/image-hub/backend/target/release/image-hub /usr/local/bin/
COPY --from=builder /build/components/image-hub/frontend/dist /app/dist

# 官方 registry 二进制（单容器内由 entrypoint 启动 registry serve；GC 亦复用该二进制）
COPY --from=registry-builder /out/registry /usr/local/bin/registry

# 单容器 entrypoint：生成 registry config → 启动 registry → 就绪后前台启动管理服务
COPY apps/com.ictrek.image-hub/image-hub-entrypoint.sh /usr/local/bin/image-hub-entrypoint.sh
RUN chmod 0755 /usr/local/bin/image-hub-entrypoint.sh

# skopeo 二进制 + 容器默认镜像信任策略 + 本地 registry insecure 配置
#   - 缺少 /etc/containers/policy.json 时 `skopeo copy` 会因
#     "no policy.json file found" 直接失败（`skopeo inspect` 不受影响）。
#   - 默认策略为 insecureAcceptAnything（不校验镜像签名），仅适用于内网受信环境。
#   - 本地 registry 为 HTTP 明文、无认证：将 127.0.0.1 / localhost 标记为 insecure，
#     使 skopeo 直连明文 HTTP，无需每次传 --dest-tls-verify=false。
#     location 按 host 前缀匹配（不带端口），故 registry 端口变更不受影响。
COPY --from=skopeo-builder /out/skopeo /usr/local/bin/skopeo
COPY --from=skopeo-builder /out/containers-policy.json /etc/containers/policy.json
COPY --from=skopeo-builder /out/registries.d-default.yaml /etc/containers/registries.d/default.yaml

RUN set -eux; \
    printf '%s\n' \
      '[[registry]]' \
      'location = "127.0.0.1"' \
      'insecure = true' \
      '' \
      '[[registry]]' \
      'location = "localhost"' \
      'insecure = true' \
      > /etc/containers/registries.conf; \
    cat /etc/containers/registries.conf

# 说明：不再对 COPY 进来的二进制执行 chmod（COPY 已保留源文件 0755 权限）——
# 对已 COPY 的文件 chmod 会在新层复制整份文件，导致镜像体积膨胀（skopeo ~27MB）。
RUN set -eux; \
    mkdir -p /var/lib/image-hub /var/lib/registry; \
    /usr/local/bin/image-hub --version || true; \
    /usr/local/bin/registry --version; \
    skopeo --version; \
    test -f /app/dist/index.html; \
    test -f /usr/local/bin/image-hub-entrypoint.sh; \
    test -f /etc/containers/policy.json; \
    grep -q 'insecure = true' /etc/containers/registries.conf

# 单容器：registry 数据面（19913）+ 管理面（19914）
EXPOSE 19913 19914

HEALTHCHECK --interval=30s --timeout=10s --start-period=15s --retries=3 \
    CMD curl -f http://localhost:19914/app/com.ictrek.image-hub/api/health || exit 1

ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/image-hub-entrypoint.sh"]
CMD ["--port", "19914", "--frontend-dir", "/app/dist", "--data-dir", "/var/lib/image-hub", "--url-base", "/app/com.ictrek.image-hub/", "--registry-url", "http://127.0.0.1:19913", "--registry-port", "19913", "--log-level", "info"]