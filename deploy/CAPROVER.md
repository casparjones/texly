# Texly — CapRover Deploy + Btrfs Snapshot Backup

This is the operator runbook for self-hosting Texly on a CapRover host with
Btrfs-snapshot backups via Snapper. The build artifacts (`Dockerfile`,
`captain-definition`) live in the repo root; the backup scripts live here in
`deploy/`.

> **Status:** Build artifacts are complete and committed. The steps below
> require shell + CapRover-dashboard access on the host and must be run by the
> operator (Frank). Run through them once, then check the "Restore test" box.

---

## 1. CapRover app

CapRover builds straight from `captain-definition` (schemaVersion 2 → uses the
repo-root `Dockerfile`).

```bash
# one-time, from a checkout of this repo on your workstation:
npm i -g caprover
caprover login                       # point at your CapRover instance
caprover deploy                      # pick the app, deploy from this branch
```

Or in the dashboard: **Apps → Create New App** (`texly`), enable
*Has Persistent Data*, then **Deployment → Method 3** (tarball) or connect the
Git repo so CapRover picks up `captain-definition` automatically.

## 2. Environment variables

Set under **App Configs → Environmental Variables**:

| Variable | Value |
|----------|-------|
| `TEXLY_JWT_SECRET` | a long random string — `openssl rand -hex 32` |
| `TEXLY_DATA_DIR` | `/data` (default, leave as-is) |
| `TEXLY_PORT` | `8080` (default; CapRover maps the container port here) |

## 3. Persistent volumes

Under **App Configs → Persistent Directories**:

| Path in container | Label / host volume |
|-------------------|---------------------|
| `/data` | `texly-data` |
| `/root/.cache/tectonic` | `texly-tectonic` |

`/data` holds `users/`, `home/`, and `share/` — this is what we back up.
`texly-tectonic` is just a regenerable package cache (no backup needed).

CapRover stores named volumes at `/var/lib/docker/volumes/<label>/_data` on the
host. If the host root fs is Btrfs (Snapper setup below assumes it), each named
volume directory lives on that filesystem.

## 4. Domain + TLS

**HTTP Settings → Connect New Domain** (e.g. `texly.example.com`), then **Enable
HTTPS** (CapRover provisions Let's Encrypt) and turn on **Force HTTPS**.
Container HTTP port stays `8080`; CapRover's nginx terminates TLS in front.

## 5. First user

On first start the log prints `No users found...`. The first user created is
auto-promoted to Superadmin (see README). After HTTPS is up:

```bash
curl -X POST https://texly.example.com/api/users \
  -H "Content-Type: application/json" \
  -d '{"username":"frank","password":"<choose>","role":"user"}'
```

Then log in at `https://texly.example.com/login`.

---

## 6. Btrfs snapshot backup (Snapper)

Run on the **host**, as root.

```bash
# 1. Install snapper
apt install snapper        # Debian/Ubuntu   (pacman -S snapper on Arch)

# 2. Register a config for the Texly data volume
snapper -c texly create-config /var/lib/docker/volumes/texly-data/_data

# 3. Apply the tuned retention from this repo
#    (merge values from deploy/snapper-texly.conf into the generated config)
$EDITOR /etc/snapper/configs/texly      # copy in TIMELINE_* / NUMBER_* values

# 4. Enable the timers (hourly timeline + daily cleanup)
systemctl enable --now snapper-timeline.timer snapper-cleanup.timer

# 5. Verify
snapper -c texly list
```

For an **off-host** copy, schedule `deploy/btrfs-backup.sh` with a destination:

```bash
sudo BACKUP_DEST=/mnt/backup/texly /path/to/repo/deploy/btrfs-backup.sh
```

That creates an "important" snapshot and `btrfs send`s it to `/mnt/backup`.

## 7. Restore test (do this once!)

```bash
snapper -c texly list                              # note a snapshot number
sudo TEXLY_CONTAINER=$(docker ps --filter name=texly -q) \
     /path/to/repo/deploy/btrfs-restore.sh <NUMBER>
```

The restore script snapshots the current state first (so the rollback is
itself reversible), stops the container, `snapper undochange`s `/data` back to
the chosen snapshot, and restarts the container. Confirm the expected project
state appears in the UI, then tick the box below.

- [ ] Deploy live on CapRover with TLS
- [ ] Persistent volumes mounted
- [ ] Snapper timeline running (`snapper -c texly list` shows hourly snapshots)
- [ ] Restore tested once and verified in the UI
