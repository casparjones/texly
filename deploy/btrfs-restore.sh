#!/usr/bin/env bash
#
# btrfs-restore.sh — restore the Texly projects volume from a Snapper snapshot.
#
# Runs ON THE CapRover HOST. Stop the Texly container first so nothing writes
# to /data mid-restore.
#
# Usage:
#   snapper -c texly list                 # find the snapshot number you want
#   sudo ./btrfs-restore.sh <NUMBER>      # restore working volume to that snapshot
#
# Strategy: rather than rsync-ing files back (slow, loses nothing-vs-deleted
# nuance), we use `snapper undochange` to roll the live subvolume back to the
# exact state of the chosen snapshot. This is reversible — the pre-restore
# state is itself snapshotted first.
set -euo pipefail

CONFIG="${SNAPPER_CONFIG:-texly}"
TARGET="${1:-}"
CONTAINER="${TEXLY_CONTAINER:-}"

if [[ $EUID -ne 0 ]]; then
  echo "error: must run as root" >&2
  exit 1
fi
if [[ -z "$TARGET" ]]; then
  echo "usage: $0 <snapshot-number>   (see: snapper -c $CONFIG list)" >&2
  exit 1
fi

echo ">> Snapshots available:"
snapper -c "$CONFIG" list

read -r -p ">> Restore config '$CONFIG' to snapshot #$TARGET? This rolls back /data. [y/N] " ans
[[ "$ans" == "y" || "$ans" == "Y" ]] || { echo "aborted."; exit 1; }

if [[ -n "$CONTAINER" ]]; then
  echo ">> Stopping container '$CONTAINER'..."
  docker stop "$CONTAINER" || true
fi

# Safety net: snapshot the current (pre-restore) state so the restore is undoable.
echo ">> Snapshotting current state before restore..."
snapper -c "$CONFIG" create --type single --cleanup-algorithm number \
  --userdata important=yes \
  --description "pre-restore safety $(date -u +%Y-%m-%dT%H:%M:%SZ)" >/dev/null

echo ">> Rolling /data back to snapshot #$TARGET..."
# undochange from the target snapshot to the current state (0) reverts the live
# subvolume to exactly what #$TARGET contained.
snapper -c "$CONFIG" undochange "${TARGET}..0"

if [[ -n "$CONTAINER" ]]; then
  echo ">> Restarting container '$CONTAINER'..."
  docker start "$CONTAINER" || true
fi

echo ">> Restore complete. Verify in the Texly UI, then check the safety snapshot"
echo "   can be deleted: snapper -c $CONFIG list"
