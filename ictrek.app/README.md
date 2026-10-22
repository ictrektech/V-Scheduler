# VOS packaging and release

A single `v-scheduler` service image contains the Rust controller/Agent and compiled UI. Controller profiles: `amd → AMD_with_cuda → amd_YYYYMMDD`, `arm → ARM_with_cuda → arm_YYYYMMDD`. The image itself requires no GPU; NVIDIA Agents use the host's supported NVIDIA runtime with `compute,utility` driver capabilities.

1. Commit/push verified application code and the VAI submodule reference.
2. Rsync the local checkout to the matching build host's `~/dev/repos/V-Scheduler`. Do not run Git on the host. The image script stages Docker inputs under `~/dev/build/V-Scheduler/images/<profile>`.
3. Run `./build_image.sh --sheet AMD_with_cuda` or `--sheet ARM_with_cuda` on that host. `--dry-run` prints the plan without reading credentials or contacting external services. Actual runs build with buildx `--load --provenance=false --sbom=false`, push the image, inspect its registry manifest, and write/read back the tag in the fixed release spreadsheet, column `v-scheduler` (hyphens). Authentication comes from the build host's write-capable `~/.feishu.json`. Rust dependencies compile offline from the SWR image `v-scheduler-rust-builder:1.97-{amd64,arm64}`, which the script uses by default; override with `SCHEDULER_RUST_BUILD_IMAGE` and `SCHEDULER_CARGO_OFFLINE`. After changing `Cargo.lock`, refresh that builder on each architecture with `docker/build_rust_builder.sh`.
4. After all required profile publications and Feishu updates succeed, run `./ictrek.app/scripts/update_version.sh patch` locally in a clean checkout. It checks local and remote trigger/public tag collisions and atomically pushes the version commit with `vos-v-scheduler-vX.Y.Z`. It never creates `vX.Y.Z`; CI owns the public tag.
5. Check the GitHub workflow, public release artifact `v_scheduler_X.Y.Z_pull.tar`, and App Store publication. Update/push the final VAI submodule reference. A triggered workflow is not a successful release.

The workflow runs protocol/frontend checks, reads images from Feishu, renders the package, creates release notes/public tag/release and publishes through the existing App Store SSH tunnel. Required secrets: `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `APPSTORE_IP`, `APPSTORE_TOKEN`. The spreadsheet ID is a fixed identifier in code, never a secret.

Registry verification uses `docker buildx imagetools inspect --raw` for both Docker schema 2 and OCI manifests, including on build hosts whose older `docker manifest inspect` command cannot read OCI images.

Configure these as repository-level Actions secrets when organization secrets are unavailable to the private repository. App Store publication runs in a separate job that downloads the published GitHub asset. If only that job fails, rerun the failed job. For an already published GitHub version, resume without rebuilding or bumping:

```sh
gh workflow run vos-release.yml --repo ictrektech/V-Scheduler --ref main -f version=0.0.1
```

Manual dispatch publishes the existing package to App Store only; it does not change the public tag or replace the GitHub asset.

Use `v0.0.3` or later: earlier packages omitted required profile descriptions and were skipped by VOS catalog parsing. Version 0.0.3 supplies `profiles[].description`, uses the supported `system` category, and adds packaging guards. Its complete manifest passes the native VOS parser, and the tc229 VOS App Store search displays the published app.

`package.sh` is for the existing CI package flow. It reads `~/.feishu.components.json`, falling back to `~/.feishu.json`, renders every template and image variable, includes Agent project templates and localized READMEs, then verifies outer/inner structure. It is not a substitute for image publication. The outer tar contains only `app.tar.gz`.

For standalone or pre-provisioned deployments, prepare `${VOS_APP_STORAGE_PATH}/config` with `controller.json` and `database.env` (`SCHEDULER_DATABASE_URL=...`), directory mode 0700 and file mode 0600. There is no extra public-directory selection. Use a dedicated PGV database/ordinary role or a separately managed PostgreSQL endpoint. Do not place credentials in public app configs. VOS users install and manage the App only. Remote nodes reuse their owning VOS app’s inventory and monitoring; no per-machine Agent installation is a user step. Remote scheduling control stays disabled until the App adapter is verified.

Initial release `v0.0.1` has AMD64/ARM64 images (`amd_20261013` / `arm_20261013`) published and recorded in Feishu, plus a GitHub pull package. Both native images passed isolated PostgreSQL migration, authenticated API and UI startup checks. App Store publication succeeded through [the existing-package dispatch](https://github.com/ictrektech/V-Scheduler/actions/runs/38341855065), confirming `com.ictrek.v-scheduler@0.0.1`; target VOS installation, authentication and multi-node deployment remain unverified.

VOS installation does not require `database.env` at Compose creation time. Missing configuration starts a first-use setup server. The UI uses the current VOS login, and the backend verifies an enabled `system_admin` through `/v1000/user/check`. PGV and Scheduler both use the VOS-injected `VOS_POSTGRES_PASSWORD` for first-use provisioning; the VOS app-detail API masks this secret and is used only to check PGV installation state. Setup creates an ordinary random owner role and isolated `v_scheduler` database. For an older PGV with a different password, run `python3 ictrek.app/scripts/prepare_pgv.py` on that VOS host, then open or retry V-Scheduler. This host-only fallback copies the running PGV credential as a mode-0600, one-use handoff in Scheduler App Storage; the controller removes it after successful setup. Setup refuses to take ownership of an unrelated database and never falls back to VOS's platform database. The VOS router is a single page, matching the actual V-Router package.

Build hosts may reuse a verified Rust 1.97 SDK cache with `SCHEDULER_RUST_BUILD_IMAGE=<local-cache-image>`; `SCHEDULER_CARGO_OFFLINE=1` refuses downloads when its Cargo registry is complete. These affect only the build stage; the runtime remains Debian and the source is copied from the synchronized checkout. The default remains the official Rust 1.97 image.

All four runtime containers inherit `pull_policy: always`. Image tags use a fixed platform/date, so multiple releases on the same day may update the same tag. VOS may pre-pull only missing images; Compose creation must also refresh existing tags so package upgrades cannot retain an older cached controller.

The Scheduler Rust process embeds Router and model management. Every profile has four containers: Scheduler plus QA, embedding and rerank Ollama. All legacy gateway/model-management aliases and host ports remain mapped to Scheduler. The shared models directory is the same installation-selected mount at `/data`; gateway state remains at `/gateway-data`. Publish only `v-scheduler` AMD/ARM images for unified service changes; reuse the original profile-specific `ollama_server` image.
