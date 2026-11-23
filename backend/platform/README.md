# Inference management sources

This directory contains the ModelInfer and Image Hub Rust services and their Vue
management pages. The import is from ictrek-monorepo commit
`c87e7605a3034384c58213f40b9041551f8b791d`. It also includes the shared error
crate, the patched SeaSchema dependency used by ModelInfer, and the original
container build definitions. The SeaSchema copy excludes its upstream CI, docs,
test fixtures, and unused synchronous crate.

The services retain separate Cargo lockfiles. ModelInfer uses SQLx 0.9 through
SeaORM, while the V-Scheduler controller uses SQLx 0.8.

On a VOS build host with Rust, `protoc`, the protobuf headers, OpenSSL development
files, and liblzma development files, use the existing lockfiles to check both
services. Set `CARGO_TARGET_DIR` under the build host's `~/dev/build` first.

```sh
cargo check --locked --all-targets --manifest-path backend/platform/components/modelinfer/backend/Cargo.toml
cargo check --locked --all-targets --manifest-path backend/platform/components/image-hub/backend/Cargo.toml
```

The V-Scheduler image builds and starts ModelInfer's `scheduler-node-manager`
binary. The scheduler controller exposes its SSH inventory, hardware scan, and
container checks under `/api/v1/managed-nodes`.

The VOS package still discovers inference deployments from a standalone
ModelInfer service. The `modelinfer` service entry point and the Image Hub
service remain separately buildable with the Dockerfiles in
`backend/platform/apps`, using `backend/platform` as the build context. Taking
over an existing deployment also requires wiring service packaging, app
authorization, scheduling, and data migration.
