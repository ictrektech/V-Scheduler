# VOS packaging and release

A single `v-scheduler` service image contains the Rust controller/Agent and compiled UI. Controller profiles: `amd → AMD_with_cuda → amd_YYYYMMDD`, `arm → ARM_with_cuda → arm_YYYYMMDD`. The image itself requires no GPU; NVIDIA Agents use the host's supported NVIDIA runtime with `utility` driver capability.

1. Commit/push verified application code and the VAI submodule reference.
2. Rsync the local checkout to the matching build host's `~/dev/repos/V-Scheduler`. Do not run Git on the host. The image script stages Docker inputs under `~/dev/build/V-Scheduler/images/<profile>`.
3. Run `./build_image.sh --sheet AMD_with_cuda` or `--sheet ARM_with_cuda` on that host. `--dry-run` prints the plan without reading credentials or contacting external services. Actual runs build with buildx `--load --provenance=false --sbom=false`, push the image, inspect its registry manifest, and write/read back the tag in the fixed release spreadsheet, column `v-scheduler` (hyphens). Authentication comes from the build host's write-capable `~/.feishu.json`.
4. After all required profile publications and Feishu updates succeed, run `./ictrek.app/scripts/update_version.sh patch` locally in a clean checkout. It checks local and remote trigger/public tag collisions and atomically pushes the version commit with `vos-v-scheduler-vX.Y.Z`. It never creates `vX.Y.Z`; CI owns the public tag.
5. Check the GitHub workflow, public release artifact `v_scheduler_X.Y.Z_pull.tar`, and App Store publication. Update/push the final VAI submodule reference. A triggered workflow is not a successful release.

The workflow runs protocol/frontend checks, reads images from Feishu, renders the package, creates release notes/public tag/release and publishes through the existing App Store SSH tunnel. Required secrets: `FEISHU_APP_ID`, `FEISHU_APP_SECRET`, `APPSTORE_IP`, `APPSTORE_TOKEN`. The spreadsheet ID is a fixed identifier in code, never a secret.

`package.sh` is for the existing CI package flow. It reads `~/.feishu.components.json`, falling back to `~/.feishu.json`, renders every template and image variable, includes Agent project templates and localized READMEs, then verifies outer/inner structure. It is not a substitute for image publication. The outer tar contains only `app.tar.gz`.

Before VOS installation, prepare a protected directory with `controller.json` and `database.env` (`SCHEDULER_DATABASE_URL=...`), and choose it through `V_SCHEDULER_CONFIG_PATH`. Use a dedicated PGV database/ordinary role or a separately managed PostgreSQL endpoint. Do not place credentials in public app configs. Deploy a matching resource-agent project on each execution node with a protected Agent config and durable journal, only after that node has this application installed.

Scripts have not been used to publish this initial app yet. No released image/package or live VOS install is implied by code checks, fixture checks or the standalone CUDA test.
