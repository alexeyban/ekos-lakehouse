#!/usr/bin/env bash
# EKOS RFC 0268 phase 3: run a Talend project's built jobs in the sandbox VM and bring back the tables
# they wrote. The jobs are third-party code: they go only to the VM, which has no route out, and the VM
# is reverted to its clean snapshot before and after the run.
#
#   run.sh <project-root> <plan.json> <out-dir>
#
# <project-root> holds the Talend project directory and the files the plan names (SQL scripts, inputs).
# The sandbox: EKOS_SANDBOX_SSH (user@host), EKOS_SANDBOX_DOMAIN (libvirt domain), EKOS_SANDBOX_SNAPSHOT.
set -euo pipefail

root_arg="${1:-}"
plan_arg="${2:-}"
out_arg="${3:-}"
if [[ -z "$root_arg" || -z "$plan_arg" || -z "$out_arg" ]]; then
  echo "usage: run.sh <project-root> <plan.json> <out-dir>" >&2
  exit 2
fi
root="$(cd "$root_arg" && pwd)"
plan="$(cd "$(dirname "$plan_arg")" && pwd)/$(basename "$plan_arg")"
mkdir -p "$out_arg"
out="$(cd "$out_arg" && pwd)"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ssh_to="${EKOS_SANDBOX_SSH:-admin@192.168.150.10}"
domain="${EKOS_SANDBOX_DOMAIN:-win11}"
snapshot="${EKOS_SANDBOX_SNAPSHOT:-ekos-sandbox-clean}"
ssh_opts=(-o BatchMode=yes -o ConnectTimeout=8)

project="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["project"])' "$plan")"
stage="$(mktemp -d)"
cleanup() {
  rm -r -- "$stage"
}
trap cleanup EXIT

# The payload: each job's built jar, the project's libraries, the files the plan names.
mkdir -p "$stage/payload/jobs" "$stage/payload/lib" "$stage/payload/project"
find "$root/$project/poms/jobs" -path '*/target/*.jar' ! -name classpath.jar -exec cp {} "$stage/payload/jobs/" \;
cp "$root/$project/temp/lib/"*.jar "$stage/payload/lib/"
find "$root/$project/poms/code" -path '*/target/*.jar' -exec cp {} "$stage/payload/lib/" \;
# A job the build compiled but did not package: its own classes, zipped (nothing is compiled here). The
# libraries the plan names from the Talend Studio the project was built with (EKOS_TALEND_STUDIO).
python3 - "$root/$project/poms/jobs" "$stage/payload" "$plan" "${EKOS_TALEND_STUDIO:-}" <<'PY'
import json, pathlib, shutil, sys, zipfile
jobs, payload, plan, studio = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), json.load(open(sys.argv[3])), sys.argv[4]
origins = []
for classes in sorted(jobs.glob("**/target/classes")):
    target = classes.parent
    if any(j.name != "classpath.jar" for j in target.glob("*.jar")):
        continue
    name = target.parent.name.replace(".", "_") + ".jar"
    with zipfile.ZipFile(payload / "jobs" / name, "w", zipfile.ZIP_DEFLATED) as z:
        for f in sorted(classes.rglob("*")):
            if f.is_file():
                z.write(f, f.relative_to(classes).as_posix())
    origins.append({"jar": name, "from": "target/classes of " + target.parent.name, "why": "the build compiled the job but did not package it"})
wanted = plan.get("studio_libs", [])
if wanted and not studio:
    sys.exit("the plan names Studio libraries: set EKOS_TALEND_STUDIO to the Talend Studio install")
for pattern in wanted:
    found = sorted(pathlib.Path(studio).glob("plugins/*/lib/" + pattern)) + sorted(pathlib.Path(studio).glob("configuration/.m2/repository/**/" + pattern))
    if not found:
        sys.exit(f"no {pattern} under {studio}")
    shutil.copy2(found[0], payload / "lib" / found[0].name)
    origins.append({"jar": found[0].name, "from": "Talend Studio: " + str(found[0].relative_to(studio)), "why": "a library the job's pom names that the project's temp/lib does not hold"})
(payload / "origins.json").write_text(json.dumps(origins, indent=1))
PY
python3 - "$plan" "$root" "$stage/payload/project" <<'PY'
import json, shutil, sys, pathlib
plan, root, dest = json.load(open(sys.argv[1])), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
for rel in plan["sql"] + [i["from"] for i in plan["inputs"]]:
    src, dst = root / rel, dest / rel
    dst.parent.mkdir(parents=True, exist_ok=True)
    (shutil.copytree if src.is_dir() else shutil.copy2)(src, dst)
PY
cp "$plan" "$stage/payload/plan.json"
cp "$here/vm-run.ps1" "$here/pg.ps1" "$stage/payload/"
tar -czf "$stage/payload.tgz" -C "$stage/payload" .
echo "payload: $(find "$stage/payload/jobs" -name '*.jar' | wc -l) job jars, $(find "$stage/payload/lib" -name '*.jar' | wc -l) libraries"

wait_ssh() {
  for _ in $(seq 1 60); do
    if timeout 8 ssh "${ssh_opts[@]}" "$ssh_to" "echo up" >/dev/null 2>&1; then
      return 0
    fi
    sleep 5
  done
  echo "the sandbox does not answer on SSH" >&2
  return 1
}

stop_vm() {
  if [[ "$(virsh domstate "$domain")" == "running" ]]; then
    timeout 20 ssh "${ssh_opts[@]}" "$ssh_to" "shutdown /s /t 2" >/dev/null 2>&1 || true
    for _ in $(seq 1 40); do
      [[ "$(virsh domstate "$domain")" == "shut off" ]] && break
      sleep 5
    done
    [[ "$(virsh domstate "$domain")" == "shut off" ]] || virsh destroy "$domain" >/dev/null
  fi
}

revert() {
  stop_vm
  virsh snapshot-revert "$domain" "$snapshot" >/dev/null
  echo "sandbox reverted to $snapshot"
}

revert
trap 'revert; cleanup' EXIT
virsh start "$domain" >/dev/null
wait_ssh
scp -q "${ssh_opts[@]}" "$stage/payload.tgz" "$ssh_to:C:/ekos-sandbox/payload.tgz"
ssh "${ssh_opts[@]}" "$ssh_to" "tar -xzf C:\\ekos-sandbox\\payload.tgz -C C:\\ekos-sandbox\\run"
echo "running the jobs in the sandbox…"
ssh "${ssh_opts[@]}" "$ssh_to" "powershell -NoProfile -ExecutionPolicy Bypass -File C:\\ekos-sandbox\\run\\vm-run.ps1" \
  > "$out/vm-run.log" 2>&1 || echo "vm-run.ps1 exited non-zero (see $out/vm-run.log)"
ssh "${ssh_opts[@]}" "$ssh_to" "tar -czf C:\\ekos-sandbox\\out.tgz -C C:\\ekos-sandbox\\run out"
scp -q "${ssh_opts[@]}" "$ssh_to:C:/ekos-sandbox/out.tgz" "$stage/out.tgz"
# The VM keeps local time in its clock, often hours off the host: no warning per file.
tar --warning=no-timestamp -xzf "$stage/out.tgz" -C "$out"
# The inputs as the jobs got them, for `lakehouse talend verify` to land the same files.
python3 - "$plan" "$root" "$out/inputs" <<'PY'
import json, shutil, sys, pathlib
plan, root, dest = json.load(open(sys.argv[1])), pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3])
for i in plan["inputs"]:
    src, dst = root / i["from"], dest / i["from"]
    dst.parent.mkdir(parents=True, exist_ok=True)
    (shutil.copytree if src.is_dir() else shutil.copy2)(src, dst, **({"dirs_exist_ok": True} if src.is_dir() else {}))
PY
cp "$plan" "$out/plan.json"
echo "results: $out/out (manifest.json, dumps/, logs/), inputs in $out/inputs"
