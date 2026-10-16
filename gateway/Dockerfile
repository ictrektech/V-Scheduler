# Build context: repository root (docker/build_image.sh runs from docker/).
# The module root is gateway/, and main.go sits at the module root.

ARG BASE_ARCH=amd64
ARG GO_IMAGE=swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:golang-1.23-alpine-20260910-${BASE_ARCH}
ARG RUNTIME_IMAGE=swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:alpine-3.20-20260910-${BASE_ARCH}

FROM ${GO_IMAGE} AS build
ARG PROXY=""
ARG APP_VERSION=dev
ENV CGO_ENABLED=0 GOFLAGS=-trimpath
WORKDIR /src
COPY gateway/go.mod ./
RUN if [ -n "$PROXY" ]; then go env -w GOPROXY="$PROXY"; fi
COPY gateway/ ./
COPY api/error-contract.json /api/error-contract.json
RUN go test ./... && go build -ldflags="-s -w -X main.version=${APP_VERSION}" -o /out/v-router .

FROM ${RUNTIME_IMAGE}
RUN addgroup -S app && adduser -S app -G app
COPY --from=build /out/v-router /usr/local/bin/v-router
ENV V_ROUTER_ADDR=:8080 V_ROUTER_STATE_PATH=/data/state.json V_ROUTER_USAGE_PATH=/data/usage.jsonl
EXPOSE 8080
USER root
ENTRYPOINT ["v-router"]
