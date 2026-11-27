# Inference management sources

This directory contains the ModelInfer, Image Hub and VOS Cluster Manager Rust services and their Vue
management pages. ModelInfer was synchronized from ictrek-monorepo commit `8b9429b93`.
The original platform import is from `c87e7605a3034384c58213f40b9041551f8b791d`. It also includes the shared error
crate, the patched SeaSchema dependency used by ModelInfer, and the original
container build definitions. The SeaSchema copy excludes its upstream CI, docs,
test fixtures, and unused synchronous crate.

VOS Cluster Manager uses ictrek-monorepo commit `3ad999b4c`, the source revision
of its `1.0.0-20261120.1.g3ad999b4` release.

The services retain separate Cargo lockfiles. ModelInfer uses SQLx 0.9 through
SeaORM, while the V-Scheduler controller uses SQLx 0.8.

On a VOS build host with Rust, `protoc`, the protobuf headers, OpenSSL development
files, and liblzma development files, use the existing lockfiles to check the
services. Set `CARGO_TARGET_DIR` under the build host's `~/dev/build` first.

```sh
cargo check --locked --all-targets --manifest-path backend/platform/components/modelinfer/backend/Cargo.toml
cargo check --locked --all-targets --manifest-path backend/platform/components/image-hub/backend/Cargo.toml
cargo check --locked --all-targets --manifest-path backend/platform/components/vos-cluster-manager/backend/Cargo.toml
```

The V-Scheduler image builds and starts ModelInfer's `scheduler-node-manager`
binary. The scheduler controller exposes its SSH inventory, hardware scan, and
container checks under `/api/v1/managed-nodes`.

The VOS package also bundles the ModelInfer deployment worker, VOS Cluster
Manager, and Image Hub. The original services remain separately buildable with
the Dockerfiles in `backend/platform/apps`, using `backend/platform` as the
build context.

The Scheduler serves the Image Hub UI at `/image-hub/`. The administrator-only
`/api/v1/image-hub/runtime/` proxy streams requests to the loopback management
listener, including large multipart imports. Registry state and blobs use
separate application storage mounts. Deploy images from this registry through
the existing Application Backends workflow by registering and using its
published address.

The scheduler image builds the ModelInfer binary and stores its UI at
`/opt/v-scheduler/inference-ui`. The ModelInfer binary accepts
`--bind-address 127.0.0.1` for an internal listener. With
`MODELINFER_PLATFORM_APP_ID=com.ictrek.v-scheduler`, it uses V-Scheduler's
`MODEL_HUB_SHARED_MODELS_PATH` as the platform model-store setting.

Administrators can inventory the legacy platform database through
`GET /api/com.ictrek.v-scheduler/scheduler/api/v1/inference/migration/preview`.
It returns table counts and deployment counts by stored state. Use the
V-Scheduler Scheduling API documentation for the response schema and errors.

ModelInfer import is available through
`POST /api/com.ictrek.v-scheduler/scheduler/api/v1/inference/migration/import`.
Before importing an existing deployment, quiesce the legacy management process
and copy its model card directory into the scheduler's persistent inference
directory. The importer compares the recorded package digests, uses a read-only
source snapshot, and commits the destination rows and import receipt together.
Conflicts roll back the whole destination transaction. After import, the
scheduler starts the embedded worker with a separate workflow queue once the
platform confirms that the old application is stopped. See the [migration
guide](../../docs/inference-migration.md) for operator steps and recovery.
