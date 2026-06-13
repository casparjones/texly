#!/usr/bin/env bash
#
# btrfs-backup.sh — take a named, important Snapper snapshot of the Texly
# projects volume and (optionally) ship it off-host with btrfs send/receive.
#
# Runs ON THE CapRover HOST, not inside the container. The Texly data lives in
# the Docker named volume "texly-data", which is itself a Btrfs subvolume at
# /var/lib/docker/volumes/texly-data/_data.
#
# Usage:
#   sudo ./btrfs-backup.sh                       # local Snapper snapshot only
#   sudo BACKUP_DEST=/mnt/backup ./btrfs-backup.sh   # + btrfs send to /mnt/backup
#
# Schedule via cron/systemd-timer for off-host copies; the in-place hourly
# timeline is handled by snapper-timeline.timer (see snapper-texly.conf).
set -euo pipefail

CONFIG="${SNAPPER_CONFIG:-texly}"
DESC="${1:-manual backup $(date -u +%Y-%m-%dT%H:%M:%SZ)}"
BACKUP_DEST="${BACKUP_DEST:-}"

if [[ $EUID -ne 0 ]]; then
  echo "error: must run as root (snapper + btrfs send need root)" >&2
  exit 1
fi

if ! command -v snapper >/dev/null 2>&1; then
  echo "error: snapper not installed (apt install snapper / pacman -S snapper)" >&2
  exit 1
fi

echo ">> Creating snapper snapshot for config '$CONFIG'..."
NUM=$(snapper -c "$CONFIG" create \
  --type single \
  --cleanup-algorithm number \
  --userdata important=yes \
  --description "$DESC" \
  --print-number)
echo ">> Created snapshot #$NUM: $DESC"

SNAP_PATH=$(snapper -c "$CONFIG" get-config | awk -F'|' '/SUBVOLUME/{gsub(/ /,"",$2);print $2}')/.snapshots/$NUM/snapshot
echo ">> Snapshot subvolume: $SNAP_PATH"

if [[ -n "$BACKUP_DEST" ]]; then
  echo ">> Shipping snapshot to $BACKUP_DEST via btrfs send/receive..."
  mkdir -p "$BACKUP_DEST"
  # Read-only snapshot is required for btrfs send; snapper snapshots are ro.
  btrfs send "$SNAP_PATH" | btrfs receive "$BACKUP_DEST"
  echo ">> Off-host copy complete: $BACKUP_DEST/snapshot"
fi

echo ">> Done. List snapshots with: snapper -c $CONFIG list"
