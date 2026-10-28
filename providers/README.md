# Provider catalog and runtime

Models.dev supplies the catalog in `models-dev/`. The existing pinned pi-ai
runtime remains in `worker.mjs`, with Rust retaining its process/credential
boundary. Native protocols, OAuth and subscription credentials are not rewritten.
Other compatible providers reuse the existing gateway proxy protocols.

`catalog.json`, `presets.json` and the native frontend selector describe the SDK
identities and URL defaults. `models-dev-providers.js` is generated public catalog
metadata, not an executable provider registry. No SDK is installed from remote
metadata. Protocol fixtures remain under `tests/`.
