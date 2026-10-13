#!/usr/bin/env python3
"""Prepare V-Scheduler's one-use PGV handoff on a VOS Docker host.

Run after both apps are installed. Secrets are read from the running PGV
container and copied directly into the scheduler's protected App Storage.
"""

import io
import json
import subprocess
import sys
import tarfile


def docker(*args, input_data=None):
    return subprocess.run(
        ["docker", *args], input=input_data, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, check=True,
    ).stdout


def one_container(names, prefix, suffix):
    matches = [name for name in names if name.startswith(prefix) and name.endswith(suffix)]
    if len(matches) != 1:
        raise RuntimeError(f"expected one running {prefix}*{suffix} container; found {len(matches)}")
    return matches[0]


def main():
    names = docker("ps", "--format", "{{.Names}}").decode().splitlines()
    pg = one_container(names, "vos-app-com-ictrek-pgv-", "-ictrek-pgv-amd-1") if any(
        name.endswith("-ictrek-pgv-amd-1") for name in names
    ) else one_container(names, "vos-app-com-ictrek-pgv-", "-ictrek-pgv-arm-1")
    controller = one_container(names, "vos-app-com-ictrek-v-scheduler-", "-controller-amd-1") if any(
        name.endswith("-controller-amd-1") and name.startswith("vos-app-com-ictrek-v-scheduler-") for name in names
    ) else one_container(names, "vos-app-com-ictrek-v-scheduler-", "-controller-arm-1")
    mounts = json.loads(docker("inspect", controller, "--format", "{{json .Mounts}}").decode())
    if not any(mount["Destination"] == "/config" and mount["Type"] == "bind" for mount in mounts):
        raise RuntimeError("scheduler has no protected /config App Storage bind mount")
    if subprocess.run(["docker", "exec", controller, "test", "-f", "/config/database.env"],
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        print("V-Scheduler database is already configured; no handoff needed")
        return
    if subprocess.run(["docker", "exec", controller, "test", "-e", "/config/pgv-bootstrap.json"],
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode == 0:
        raise RuntimeError("a PGV handoff already exists; inspect setup status before replacing it")
    raw_env = json.loads(docker("inspect", pg, "--format", "{{json .Config.Env}}").decode())
    environment = dict(item.split("=", 1) for item in raw_env if "=" in item)
    for key in ("POSTGRES_USER", "POSTGRES_PASSWORD", "POSTGRES_DB"):
        if not environment.get(key):
            raise RuntimeError(f"running PGV does not expose {key}")
    payload = json.dumps({
        "POSTGRES_SUPERUSER": environment["POSTGRES_USER"],
        "POSTGRES_PASSWORD": environment["POSTGRES_PASSWORD"],
        "POSTGRES_DB": environment["POSTGRES_DB"],
    }).encode()
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as archive:
        entry = tarfile.TarInfo("pgv-bootstrap.json")
        entry.size = len(payload)
        entry.mode = 0o600
        entry.uid = entry.gid = 0
        archive.addfile(entry, io.BytesIO(payload))
    docker("cp", "-", f"{controller}:/config/", input_data=stream.getvalue())
    print("PGV handoff prepared in protected App Storage; open V-Scheduler in VOS to finish setup")


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.CalledProcessError) as error:
        # Docker stderr can include environment values; never print it here.
        print(f"PGV handoff failed: {type(error).__name__}", file=sys.stderr)
        sys.exit(1)
