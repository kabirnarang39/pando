#!/bin/bash
# End-to-end check of `pando mount` on macOS: serve a small repository, mount it over NFS,
# and compare the mount with a normal checkout byte for byte. Tries an unprivileged mount
# first and falls back to `sudo mount_nfs` (CI runners allow it).
#   scripts/test-mount-macos.sh [BIN_DIR]     (default: target/release)
set -euo pipefail
BIN="$(cd "${1:-target/release}" && pwd)"
W="$(mktemp -d)"
trap 'umount "$W/mnt" 2>/dev/null || sudo -n umount "$W/mnt" 2>/dev/null || true; kill $(jobs -p) 2>/dev/null || true' EXIT

mkdir -p "$W/src/docs/deep/er" "$W/src/bin" "$W/mnt"
for i in $(seq 1 40); do echo "line $i" > "$W/src/docs/f$i.txt"; done
echo nested > "$W/src/docs/deep/er/file.md"
head -c 12000000 /dev/urandom > "$W/src/big.bin"
printf '#!/bin/sh\necho hi\n' > "$W/src/bin/run" && chmod +x "$W/src/bin/run"
ln -s ../docs/f1.txt "$W/src/bin/link"
git -C "$W/src" init -q && git -C "$W/src" add -A && git -C "$W/src" -c user.name=t -c user.email=t@t commit -qm init

"$BIN/pandod" server --listen 127.0.0.1:7499 --metadata "sqlite://$W/meta.db" >"$W/server.log" 2>&1 &
sleep 2
"$BIN/pando" import-git "$W/src" --server http://127.0.0.1:7499 >/dev/null
"$BIN/pando" clone http://127.0.0.1:7499 "$W/clone" >/dev/null

if "$BIN/pando" mount "$W/mnt" --server http://127.0.0.1:7499 >"$W/mount.log" 2>&1 & sleep 4 && mount | grep -q "$W/mnt"; then
  echo "unprivileged mount: works"
else
  echo "unprivileged mount: refused ($(grep -o 'Operation not permitted' "$W/mount.log" | head -1 || true)); using sudo"
  "$BIN/pando" mount "$W/mnt" --server http://127.0.0.1:7499 --serve-only >"$W/serve.out" 2>"$W/serve.err" &
  sleep 3
  sudo -n $(cat "$W/serve.out")
fi

diff -r --exclude=.pando "$W/clone" "$W/mnt"
[ -x "$W/mnt/bin/run" ] && [ "$(readlink "$W/mnt/bin/link")" = ../docs/f1.txt ]
cmp "$W/src/big.bin" "$W/mnt/big.bin"
! touch "$W/mnt/new-file" 2>/dev/null
echo "mount matches checkout: $(find "$W/mnt" -type f | wc -l | tr -d ' ') files, symlink and exec bit intact, read-only"
