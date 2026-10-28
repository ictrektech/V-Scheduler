#!/usr/bin/env python3
"""Regenerate the checked-in metadata and UI fallback from a verified local input."""
import collections
import gzip
import hashlib
import json
import pathlib
import sys
from datetime import date

root = pathlib.Path(__file__).resolve().parent.parent
raw = pathlib.Path(sys.argv[1]).read_bytes()
if len(raw) > 16 * 1024 * 1024:
    raise ValueError("catalog exceeds 16 MiB")
catalog = json.loads(raw)
if not isinstance(catalog, dict) or not catalog:
    raise ValueError("expected provider object")
rows = []
for identifier, provider in catalog.items():
    models = provider.get("models")
    if not isinstance(models, dict):
        raise ValueError("expected v2 provider models")
    bases = collections.Counter(
        api["base_url"] for model in models.values()
        for api in model.get("api", {}).values() if api.get("base_url")
    )
    # Match Rust's lexical tie breaker, so the UI's default path is recognized.
    base = max(bases, key=lambda value: (bases[value], value)) if bases else ""
    portable = {}
    if identifier not in ('amazon-bedrock', 'google-vertex', 'github-copilot', 'azure', 'cloudflare-ai-gateway'):
        for api_name, protocol in [('chat-completions', 'chat_completions'), ('messages', 'anthropic')]:
            candidates = collections.Counter(model.get('api', {}).get(api_name, {}).get('base_url', '') for model in models.values())
            candidates.pop('', None)
            if candidates:
                portable = dict(registrationProtocol=protocol, registrationBase=max(candidates, key=lambda value: (candidates[value], value)))
                break
    rows.append(dict(**portable, id=identifier, name=provider.get("name", identifier), base=base,
                     env=provider.get("env", []), models_count=len(models)))
rows.sort(key=lambda p: p["name"].casefold())
folder = root / "providers/models-dev"
(folder / "snapshot.json.gz").write_bytes(gzip.compress(raw, mtime=0))
(folder / "snapshot-source.json").write_text(json.dumps(dict(
    source="Models.dev", format="catalog-v2", retrieved_at=str(date.today()),
    snapshot_url="https://github.com/anomalyco/opencode/blob/dev/packages/core/src/models-dev/snapshot.txt",
    sha256=hashlib.sha256(raw).hexdigest(), providers=len(catalog),
    models=sum(len(p["models"]) for p in catalog.values())), indent=2) + "\n")
(root / "frontend/src/gateway/models-dev-providers.js").write_text(
    "// Generated from the bundled Models.dev snapshot; refreshed from the Rust API.\n"
    "export const MODELS_DEV_PROVIDERS = " + json.dumps(rows, ensure_ascii=False, indent=2) + ";\n")
print(f"Updated {len(catalog)} providers; no runtime dependencies installed")
