ARG RUST_BUILD_IMAGE=rust:1.97-bookworm
ARG SCHEDULER_APPLICATION_BASE=debian:bookworm-slim
FROM node:24-bookworm-slim AS ui
WORKDIR /build/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
COPY assets/ /build/assets/
COPY docs/model-weight-transfer.md /build/docs/model-weight-transfer.md
RUN npm run build

FROM node:24-bookworm-slim AS inference-ui
WORKDIR /build/modelinfer
COPY backend/platform/components/modelinfer/frontend/package.json backend/platform/components/modelinfer/frontend/pnpm-lock.yaml backend/platform/components/modelinfer/frontend/pnpm-workspace.yaml ./
RUN npm install -g pnpm@11.10.0 && pnpm install --frozen-lockfile
COPY backend/platform/components/modelinfer/frontend/ ./
COPY frontend/src/vos-auth.ts frontend/src/diagnostics.js /build/scheduler/
COPY --from=ui /build/frontend/node_modules/@noble/hashes /build/scheduler/node_modules/@noble/hashes
RUN VITE_SCHEDULER_INFERENCE=true pnpm build

FROM node:24-bookworm-slim AS cluster-ui
WORKDIR /build/cluster
COPY backend/platform/components/vos-cluster-manager/frontend/package.json backend/platform/components/vos-cluster-manager/frontend/pnpm-lock.yaml backend/platform/components/vos-cluster-manager/frontend/pnpm-workspace.yaml ./
RUN npm install -g pnpm@11.10.0 && pnpm install --frozen-lockfile
COPY backend/platform/components/vos-cluster-manager/frontend/ ./
COPY frontend/src/vos-auth.ts frontend/src/diagnostics.js /build/scheduler/
COPY --from=ui /build/frontend/node_modules/@noble/hashes /build/scheduler/node_modules/@noble/hashes
RUN VITE_SCHEDULER_CLUSTER=true pnpm build

FROM node:24-bookworm-slim AS image-hub-ui
WORKDIR /build/image-hub
COPY backend/platform/components/image-hub/frontend/package.json backend/platform/components/image-hub/frontend/pnpm-lock.yaml backend/platform/components/image-hub/frontend/pnpm-workspace.yaml ./
RUN npm install -g pnpm@11.10.0 && pnpm install --frozen-lockfile
COPY backend/platform/components/image-hub/frontend/ ./
COPY frontend/src/vos-auth.ts frontend/src/diagnostics.js /build/scheduler/
COPY --from=ui /build/frontend/node_modules/@noble/hashes /build/scheduler/node_modules/@noble/hashes
RUN VITE_SCHEDULER_IMAGE_HUB=true pnpm build

FROM golang:1.26-bookworm AS skopeo-builder
ARG SKOPEO_VERSION=1.24.1
ARG SKOPEO_SOURCE_URL=https://codeload.github.com/containers/skopeo/tar.gz/refs/tags/v${SKOPEO_VERSION}
ARG SKOPEO_SOURCE_SHA256=6a3f342a327ec7684198eb0e15114940b6101506db48d9a96590f5942cb3b335

# 源码构建依赖（install.md「Building without a container」Debian/Ubuntu 清单）
RUN set -eux; \
    apt-get update; \
    apt-get install -y --no-install-recommends \
      libgpgme-dev libassuan-dev libbtrfs-dev libsqlite3-dev pkg-config make gcc curl ca-certificates; \
    rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/skopeo-build

# 下载 + 校验 + 解压（curl 为 vos-builder 内置）
RUN set -eux; \
    curl -fsSL --retry 3 -o skopeo.tar.gz "${SKOPEO_SOURCE_URL}"; \
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

FROM debian:bookworm-slim AS registry-builder
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
ARG REGISTRY_VERSION=3.1.1
ARG REGISTRY_RELEASE_URL=https://github.com/distribution/distribution/releases/download
ARG REGISTRY_SHA256_AMD64=6f330a3ba9ea1d23a6ee189f449d792595240585bb2f159123d76ac594f70dd8
ARG REGISTRY_SHA256_ARM64=8167316d2b4a57e10d44f8c8a3c75fea5f3ec1c71872760bb903e5e8e52e9ad6

RUN set -eux; \
    arch="$(uname -m)"; \
    case "${arch}" in \
      x86_64) goarch=amd64; sha="${REGISTRY_SHA256_AMD64}" ;; \
      aarch64|arm64) goarch=arm64; sha="${REGISTRY_SHA256_ARM64}" ;; \
      *) echo "不支持的构建架构: ${arch}" >&2; exit 1 ;; \
    esac; \
    url="${REGISTRY_RELEASE_URL}/v${REGISTRY_VERSION}/registry_${REGISTRY_VERSION}_linux_${goarch}.tar.gz"; \
    curl -fsSL --retry 3 -o /tmp/registry.tar.gz "${url}"; \
    echo "${sha}  /tmp/registry.tar.gz" | sha256sum -c -; \
    mkdir -p /tmp/registry-extract; \
    tar xzf /tmp/registry.tar.gz -C /tmp/registry-extract; \
    test -f /tmp/registry-extract/registry; \
    mkdir -p /out; \
    install -m 0755 /tmp/registry-extract/registry /out/registry; \
    /out/registry --version

FROM node:24-bookworm-slim AS providers
WORKDIR /build/providers
COPY providers/package*.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY providers/worker.mjs providers/bridge.mjs providers/catalog.json providers/presets.json ./
COPY providers/LICENSE.pi-reference ./LICENSE.pi-reference

FROM ${RUST_BUILD_IMAGE} AS scheduler-core
WORKDIR /build/backend
RUN apt-get -o Acquire::Retries=3 update && apt-get install -y --no-install-recommends \
    protobuf-compiler libprotobuf-dev libssl-dev liblzma-dev && rm -rf /var/lib/apt/lists/*
COPY backend/ ./
COPY api/ /build/api/
COPY providers/catalog.json /build/providers/catalog.json
COPY providers/models-dev/ /build/providers/models-dev/
ENV CARGO_TARGET_DIR=/build/target
ARG CARGO_OFFLINE=false
RUN if [ "$CARGO_OFFLINE" = true ]; then cargo build --release --locked --offline; else cargo build --release --locked; fi
FROM ${SCHEDULER_APPLICATION_BASE} AS scheduler-application
COPY --from=scheduler-core /build/target/release/v-scheduler /usr/local/bin/v-scheduler
RUN rm -rf /opt/v-scheduler/ui
COPY --from=ui /build/frontend/dist /opt/v-scheduler/ui
COPY config/app-calibrations.json /opt/v-scheduler/app-calibrations.json

FROM scheduler-core AS backend
RUN if [ "$CARGO_OFFLINE" = true ]; then set -- --offline; else set --; fi; \
    cargo build --release --locked "$@" \
      --manifest-path platform/components/modelinfer/backend/Cargo.toml \
      --bin modelinfer --bin scheduler-node-manager
RUN if [ "$CARGO_OFFLINE" = true ]; then set -- --offline; else set --; fi; \
    VOS_CLUSTER_MANAGER_OWNER_APP=com.ictrek.v-scheduler cargo build --release --locked "$@" \
      --manifest-path platform/components/vos-cluster-manager/backend/Cargo.toml

RUN if [ "$CARGO_OFFLINE" = true ]; then set -- --offline; else set --; fi; \
    cargo build --release --locked "$@" --manifest-path platform/components/image-hub/backend/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=3 -o Acquire::http::Timeout=30 update && apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=3 -o Acquire::http::Timeout=30 install -y --no-install-recommends ca-certificates curl bzip2 xz-utils libstdc++6 keepalived iproute2 libgpgme11 libsqlite3-0 && rm -rf /var/lib/apt/lists/*
COPY --from=providers /usr/local/bin/node /usr/local/bin/node
COPY --from=providers /usr/local/LICENSE /opt/v-scheduler/providers/NODE-LICENSE
COPY --from=providers /build/providers /opt/v-scheduler/providers
COPY providers/models-dev/LICENSE.* /opt/v-scheduler/providers/licenses/
COPY --from=backend /build/target/release/v-scheduler /usr/local/bin/v-scheduler
COPY --from=backend /build/target/release/scheduler-node-manager /usr/local/bin/scheduler-node-manager
COPY --from=backend /build/target/release/image-hub /usr/local/bin/image-hub
COPY --from=registry-builder /out/registry /usr/local/bin/registry
COPY --from=skopeo-builder /out/skopeo /usr/local/bin/skopeo
COPY --from=skopeo-builder /out/containers-policy.json /etc/containers/policy.json
COPY --from=skopeo-builder /out/registries.d-default.yaml /etc/containers/registries.d/default.yaml
COPY --chmod=755 backend/platform/apps/com.ictrek.image-hub/image-hub-entrypoint.sh /usr/local/bin/image-hub-entrypoint.sh
COPY --from=backend /build/target/release/modelinfer /usr/local/bin/modelinfer
COPY --from=backend /build/target/release/vos-cluster-manager /usr/local/bin/vos-cluster-manager
COPY --from=backend /build/target/release/vcm-etcd-auth /usr/local/bin/vcm-etcd-auth
COPY --chmod=755 backend/platform/components/vos-cluster-manager/keepalived/keepalived-entrypoint.sh /usr/local/bin/keepalived-entrypoint.sh
COPY --chmod=755 backend/platform/components/vos-cluster-manager/keepalived/notify.sh /etc/keepalived/notify.sh
COPY --from=ui /build/frontend/dist /opt/v-scheduler/ui
COPY --from=inference-ui /build/modelinfer/dist /opt/v-scheduler/inference-ui
COPY --from=image-hub-ui /build/image-hub/dist /opt/v-scheduler/image-hub-ui
COPY --from=cluster-ui /build/cluster/dist /opt/v-scheduler/cluster-ui
COPY config/voice-calibrations.json /opt/v-scheduler/voice-calibrations.json
COPY config/app-calibrations.json /opt/v-scheduler/app-calibrations.json
COPY config/vsub-calibrations.json /opt/v-scheduler/vsub-calibrations.json
ENTRYPOINT ["v-scheduler"]
CMD ["controller", "--config", "/config/controller.json", "--listen", "0.0.0.0:38120", "--ui-dir", "/opt/v-scheduler/ui"]
