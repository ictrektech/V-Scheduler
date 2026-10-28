# Provider metadata

`snapshot.json.gz` is a bounded, deterministic gzip of the Models.dev v2 snapshot
shipped by OpenCode. `snapshot-source.json` records the download URL, date and
uncompressed SHA-256. Both upstream MIT licenses are included. Metadata is never
loaded as executable code; SDK/npm names do not install or execute packages.

The Rust service loads this snapshot offline, optionally restores its last valid
private cache, and refreshes `https://models.dev/experimental/v2.0/api.json?type=chat,decision,image` every six hours.
An invalid, oversized or unavailable update retains the preceding catalog.
`V_SCHEDULER_MODELS_DEV_URL` overrides only the metadata source, not provider auth
or inference endpoints. Existing URLs, IDs and credentials are not rewritten.

To refresh the bundled data explicitly, run from the repository root:

```sh
python3 providers/sync_models_dev.py /path/to/verified-models-dev-snapshot.json
```

The script also regenerates the public frontend fallback selector. The server's
`GET /api/v1/provider-catalog` is the live catalog used by the UI. Catalog presence
is metadata, not proof of model access, availability or successful live inference.
Non-chat metadata is retained for reference. Existing pi-ai native transports
and compatible OpenAI/Anthropic proxies remain authoritative for invocation.
Ollama, TypeSafe/Jev, image/audio and business adapters keep their own discovery.
