# Python Containerized App (uv) — Monorepo

Full-stack task management app demonstrating containerization with
[uv](https://docs.astral.sh/uv/) for the Python backend and Angular 19
for the frontend, following **hexagonal architecture**.

## Monorepo layout

```
python-containerized-app/
├── compose.yml           # Orchestrates all 3 services
├── backend/              # Python FastAPI + PostgreSQL
│   ├── Dockerfile
│   ├── pyproject.toml
│   ├── uv.lock
│   ├── src/app/
│   │   ├── main.py                     # Composition root
│   │   ├── config.py                   # Externalized settings
│   │   ├── domain/
│   │   │   ├── models.py               # Pure dataclass entities
│   │   │   └── ports.py                # Protocol interfaces
│   │   └── adapters/
│   │       ├── api/                    # FastAPI (driving adapter)
│   │       └── persistence/            # SQLAlchemy (driven adapter)
│   └── tests/
├── frontend/             # Angular 19 SPA
│   ├── Dockerfile
│   ├── nginx.conf        # Serves SPA + reverse-proxies /api → backend
│   ├── package.json
│   └── src/app/
│       ├── models/
│       ├── services/
│       └── components/
└── README.md
```

## Quick start

```bash
docker compose up --build
# Frontend → http://localhost:4200
# Backend API → http://localhost:8000/docs
```

## Services

| Service    | Port | Description                                      |
|------------|------|--------------------------------------------------|
| `frontend` | 4200 | Angular SPA served by nginx, proxies `/api` to backend |
| `backend`  | 8000 | FastAPI with Swagger UI at `/docs`               |
| `db`       | 5432 | PostgreSQL 17                                    |

## Configuration

Backend settings are read from environment variables (or `.env`):

| Variable       | Default                                          |
|----------------|--------------------------------------------------|
| `DATABASE_URL` | `postgresql+asyncpg://app:app@localhost:5432/app` |
| `PORT`         | `8000`                                           |
| `APP_VERSION`  | `0.1.0`                                          |
| `ECHO_SQL`     | `false`                                          |

## API

| Method | Path          | Description                       |
|--------|---------------|-----------------------------------|
| GET    | `/`           | App info                          |
| GET    | `/health`     | Health check (includes DB status) |
| POST   | `/tasks`      | Create a task                     |
| GET    | `/tasks`      | List tasks (`?done=true/false`)   |
| GET    | `/tasks/{id}` | Get a task                        |
| PATCH  | `/tasks/{id}` | Update a task                     |
| DELETE | `/tasks/{id}` | Delete a task                     |

## Tests

```bash
cd backend && uv run pytest tests/ -v
```
