ARG RUST_BUILD_IMAGE=rust:1.97-bookworm
FROM node:24-bookworm-slim AS ui
WORKDIR /build/frontend
COPY frontend/package*.json ./
RUN npm ci
COPY frontend/ ./
COPY assets/ /build/assets/
RUN npm run build

FROM ${RUST_BUILD_IMAGE} AS backend
WORKDIR /build/backend
COPY backend/ ./
COPY api/ /build/api/
ENV CARGO_TARGET_DIR=/build/target
ARG CARGO_OFFLINE=false
RUN if [ "$CARGO_OFFLINE" = true ]; then cargo build --release --locked --offline; else cargo build --release --locked; fi

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/*
COPY --from=backend /build/target/release/v-scheduler /usr/local/bin/v-scheduler
COPY --from=ui /build/frontend/dist /opt/v-scheduler/ui
COPY config/voice-calibrations.json /opt/v-scheduler/voice-calibrations.json
ENTRYPOINT ["v-scheduler"]
CMD ["controller", "--config", "/config/controller.json", "--listen", "0.0.0.0:38120", "--ui-dir", "/opt/v-scheduler/ui"]
