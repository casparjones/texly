# Texly

A self-hosted LaTeX editor with a web-based interface, built with Rust (Axum) and Tectonic as the LaTeX engine. Supports multiple users with role-based access control, a CodeMirror 6 editor, and integrated PDF preview.

---

## Volumes

These are the directories you need to mount as persistent volumes when deploying:

| Path | Description |
|------|-------------|
| `/data/users` | User accounts (one `.toml` file per user) |
| `/data/home` | User home directories containing LaTeX projects |
| `/data/share` | Shared folder accessible by all users |
| `/root/.cache/tectonic` | Tectonic package cache — downloaded once, used offline afterwards |

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `TEXLY_JWT_SECRET` | **required** | Secret key for JWT signing (use a long random string) |
| `TEXLY_DATA_DIR` | `/data` | Root directory for users/, home/, share/ |
| `TEXLY_PORT` | `8080` | HTTP port to listen on |
| `TEXLY_STATIC_DIR` | `./static` | Path to the static/ folder |

Example with Docker:
```bash
docker run -d \
  -p 8080:8080 \
  -e TEXLY_JWT_SECRET=your-very-long-secret-here \
  -v texly-data:/data \
  -v texly-tectonic:/root/.cache/tectonic \
  texly:latest
```

---

## First Start / First User

Texly has no built-in default users. On first start, the server logs:

```
No users found. Create the first user via POST /api/users — it will automatically be Superadmin.
```

**The first user created via `POST /api/users` will automatically become Superadmin**, regardless of the role field in the request body. This means the first user creation does not require authentication.

Example using curl:
```bash
curl -X POST http://localhost:8080/api/users \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "changeme", "role": "user"}'
```

After that, log in at `http://localhost:8080/login`.

---

## Coolify Deployment

1. Add Texly as a **Docker image** resource in Coolify (use the GitHub repo or a pre-built image).
2. Set the required environment variable `TEXLY_JWT_SECRET` to a random string (at least 32 characters).
3. Add **persistent volumes**:
   - `/data` → for users, home directories, and share
   - `/root/.cache/tectonic` → for the Tectonic package cache
4. Set the **port** to `8080` (or override with `TEXLY_PORT`).
5. Deploy. On first access, create the initial superadmin user via the API or the login page.

> **Tip**: The first compile will download LaTeX packages from the internet. Subsequent compiles use the cache and work offline.

---

## Architecture

- **Backend**: Rust with Axum 0.7 — REST API, JWT auth via HttpOnly cookies
- **Frontend**: Vanilla JavaScript SPA with CodeMirror 6 (LaTeX syntax) and PDF.js
- **LaTeX Engine**: [Tectonic](https://tectonic-typesetting.github.io/) (runs as subprocess)
- **Auth**: JWT in `texly_session` HttpOnly cookie, 7-day expiry, Argon2id password hashing
- **File storage**: Plain filesystem under `$TEXLY_DATA_DIR`
