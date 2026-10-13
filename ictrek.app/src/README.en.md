# V-Scheduler Resource Scheduling

Version __APP_VERSION__

Install one controller with a dedicated PostgreSQL database (PGV recommended). Configuration lives in the dedicated ${VOS_APP_STORAGE_PATH}/config directory; no public configuration directory selection is needed. Before installation, provision controller.json and database.env (SCHEDULER_DATABASE_URL) there, with directory mode 0700 and file mode 0600. Never place credentials in public configuration fields.

The controller uses the approved static node/executor registry. Open the management page with an administrator service token. UI language follows VOS by default (English / Simplified Chinese / Traditional Chinese).

Deploy resource-agent-amd or resource-agent-arm on each execution node, with AGENT_CONFIG_PATH pointing to protected agent.json and journal set to /journal/agent.json. Set AGENT_RUNTIME=nvidia on supported NVIDIA nodes; CPU nodes use runc. The V-Scheduler package must already be installed on that node. Agents connect to the controller and registered executors, never directly to PostgreSQL.

App installation and configured execution location are distinct. Cloud inference bypasses local permits. Unconfirmed execution retains its resource reservation; deletion only removes stopped task records and does not delete business outputs.

This initial package does not implement automatic enrollment, automatic Agent deployment, OIDC management login, HA or device-level multi-GPU allocation.
