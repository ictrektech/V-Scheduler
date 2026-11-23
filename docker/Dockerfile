ARG RUST_BUILD_IMAGE=rust:1.97-bookworm
FROM node:24-bookworm-slim AS ui
WORKDIR /build/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
COPY assets/ /build/assets/
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

FROM node:24-bookworm-slim AS providers
WORKDIR /build/providers
COPY providers/package*.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY providers/worker.mjs providers/bridge.mjs providers/catalog.json providers/presets.json ./
COPY providers/LICENSE.pi-reference ./LICENSE.pi-reference

FROM ${RUST_BUILD_IMAGE} AS backend
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
RUN if [ "$CARGO_OFFLINE" = true ]; then set -- --offline; else set --; fi; \
    cargo build --release --locked "$@" \
      --manifest-path platform/components/modelinfer/backend/Cargo.toml \
      --bin modelinfer --bin scheduler-node-manager
RUN if [ "$CARGO_OFFLINE" = true ]; then set -- --offline; else set --; fi; \
    VOS_CLUSTER_MANAGER_OWNER_APP=com.ictrek.v-scheduler cargo build --release --locked "$@" \
      --manifest-path platform/components/vos-cluster-manager/backend/Cargo.toml

FROM debian:bookworm-slim
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=3 -o Acquire::http::Timeout=30 update && apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=3 -o Acquire::http::Timeout=30 install -y --no-install-recommends ca-certificates curl bzip2 xz-utils libstdc++6 keepalived iproute2 && rm -rf /var/lib/apt/lists/*
COPY --from=providers /usr/local/bin/node /usr/local/bin/node
COPY --from=providers /usr/local/LICENSE /opt/v-scheduler/providers/NODE-LICENSE
COPY --from=providers /build/providers /opt/v-scheduler/providers
COPY providers/models-dev/LICENSE.* /opt/v-scheduler/providers/licenses/
COPY --from=backend /build/target/release/v-scheduler /usr/local/bin/v-scheduler
COPY --from=backend /build/target/release/scheduler-node-manager /usr/local/bin/scheduler-node-manager
COPY --from=backend /build/target/release/modelinfer /usr/local/bin/modelinfer
COPY --from=backend /build/target/release/vos-cluster-manager /usr/local/bin/vos-cluster-manager
COPY --from=backend /build/target/release/vcm-etcd-auth /usr/local/bin/vcm-etcd-auth
COPY --chmod=755 backend/platform/components/vos-cluster-manager/keepalived/keepalived-entrypoint.sh /usr/local/bin/keepalived-entrypoint.sh
COPY --chmod=755 backend/platform/components/vos-cluster-manager/keepalived/notify.sh /etc/keepalived/notify.sh
COPY --from=ui /build/frontend/dist /opt/v-scheduler/ui
COPY --from=inference-ui /build/modelinfer/dist /opt/v-scheduler/inference-ui
COPY --from=cluster-ui /build/cluster/dist /opt/v-scheduler/cluster-ui
COPY config/voice-calibrations.json /opt/v-scheduler/voice-calibrations.json
COPY config/app-calibrations.json /opt/v-scheduler/app-calibrations.json
COPY config/vsub-calibrations.json /opt/v-scheduler/vsub-calibrations.json
ENTRYPOINT ["v-scheduler"]
CMD ["controller", "--config", "/config/controller.json", "--listen", "0.0.0.0:38120", "--ui-dir", "/opt/v-scheduler/ui"]
