ARG RUST_BUILD_IMAGE=rust:1.97-bookworm
FROM node:24-bookworm-slim AS ui
WORKDIR /build/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
COPY assets/ /build/assets/
RUN npm run build

FROM node:24-bookworm-slim AS providers
WORKDIR /build/providers
COPY providers/package*.json ./
RUN npm ci --omit=dev --ignore-scripts
COPY providers/worker.mjs providers/bridge.mjs providers/catalog.json providers/presets.json ./

FROM ${RUST_BUILD_IMAGE} AS backend
WORKDIR /build/backend
COPY backend/ ./
COPY api/ /build/api/
COPY providers/catalog.json /build/providers/catalog.json
ENV CARGO_TARGET_DIR=/build/target
ARG CARGO_OFFLINE=false
RUN if [ "$CARGO_OFFLINE" = true ]; then cargo build --release --locked --offline; else cargo build --release --locked; fi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl bzip2 xz-utils libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY --from=providers /usr/local/bin/node /usr/local/bin/node
COPY --from=providers /usr/local/LICENSE /opt/v-scheduler/providers/NODE-LICENSE
COPY --from=providers /build/providers /opt/v-scheduler/providers
COPY --from=backend /build/target/release/v-scheduler /usr/local/bin/v-scheduler
COPY --from=ui /build/frontend/dist /opt/v-scheduler/ui
COPY config/voice-calibrations.json /opt/v-scheduler/voice-calibrations.json
COPY config/app-calibrations.json /opt/v-scheduler/app-calibrations.json
COPY config/vsub-calibrations.json /opt/v-scheduler/vsub-calibrations.json
ENTRYPOINT ["v-scheduler"]
CMD ["controller", "--config", "/config/controller.json", "--listen", "0.0.0.0:38120", "--ui-dir", "/opt/v-scheduler/ui"]
