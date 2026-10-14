# V-Scheduler Resource Scheduling

Version __APP_VERSION__

Installation checks the PGV dependency. On first opening, sign in as a VOS administrator to automatically initialize a dedicated database, ordinary database role and internal collector identity. Configuration lives in the private ${VOS_APP_STORAGE_PATH}/config directory. No configuration files or terminal commands are required. The page shows initialization status while PGV is unavailable.

The controller uses the approved static node/executor registry. Sign in with your VOS administrator account. UI language follows VOS by default (English / Simplified Chinese / Traditional Chinese).

Host resource collection is built into this app and starts automatically after installation. No separate Agent, terminal command or collector credential is required. Expand the logical processor count to inspect each processor. Jetson Thor uses one shared system RAM pool for CPU and GPU.

App installation and configured execution location are distinct. Cloud inference bypasses local permits. Unconfirmed execution retains its resource reservation; deletion only removes stopped task records and does not delete business outputs.

VOS administrator sign-in and automatic local collection are supported. HA and device-level multi-GPU allocation are not implemented.

The dashboard shows 24-hour queue wait, occupancy P50/P95, task failure rate and model permit history. Live task details include a status timeline, per-attempt assignment/stop times, waiting guidance and controlled retry. Combine status, application and node filters, search application task IDs, and load older tasks beyond the initial 200. P95 requires at least 20 valid samples; old permits retain unknown release times.
