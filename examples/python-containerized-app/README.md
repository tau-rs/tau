# Python Containerized App (uv)

Dummy FastAPI + PostgreSQL application demonstrating Python
containerization with [uv](https://docs.astral.sh/uv/) as the package
manager, following **hexagonal architecture** (ports & adapters).

## What this shows

- **Hexagonal architecture** — domain logic is framework-free; the API
  and database are swappable adapters wired through ports (protocols).
- **Externalized configuration** — `pydantic-settings` loads DB URL,
  port, and version from environment variables or `.env` file.
- **`pyproject.toml`** — standard Python project metadata; uv reads it
  natively (no `requirements.txt`).
- **Multi-stage `Dockerfile`** — dependencies are installed in a cached
  layer; only the app source triggers a rebuild on code changes.
- **`compose.yml`** — app + PostgreSQL with health checks, persistent
  volume, and `depends_on` ordering.

## Quick start

```bash
# Generate the lock file first (required by --frozen in the Dockerfile)
uv lock

# Build and run app + database
docker compose up --build
# → http://localhost:8000
# → http://localhost:8000/docs  (Swagger UI)
```

## Configuration

All settings are read from environment variables (or a `.env` file):

| Variable       | Default                                          | Description       |
|----------------|--------------------------------------------------|-------------------|
| `DATABASE_URL` | `postgresql+asyncpg://app:app@localhost:5432/app` | SQLAlchemy async URL |
| `PORT`         | `8000`                                           | Server port       |
| `APP_VERSION`  | `0.1.0`                                          | Reported in /health |
| `ECHO_SQL`     | `false`                                          | Log SQL queries   |

## API

| Method   | Path             | Description                        |
|----------|------------------|------------------------------------|
| GET      | `/`              | App info                           |
| GET      | `/health`        | Health check (includes DB status)  |
| POST     | `/tasks`         | Create a task                      |
| GET      | `/tasks`         | List tasks (filter: `?done=true`)  |
| GET      | `/tasks/{id}`    | Get a single task                  |
| PATCH    | `/tasks/{id}`    | Update a task                      |
| DELETE   | `/tasks/{id}`    | Delete a task                      |
| GET      | `/docs`          | Swagger UI                         |

## Architecture

```
src/app/
├── main.py                         # Composition root — wires adapters to ports
├── config.py                       # Externalized settings (pydantic-settings)
├── domain/
│   ├── models.py                   # Pure domain entities (dataclasses)
│   └── ports.py                    # Port interfaces (Protocol)
└── adapters/
    ├── api/
    │   ├── schemas.py              # Pydantic request/response DTOs
    │   └── routes.py               # FastAPI router (driving adapter)
    └── persistence/
        ├── database.py             # Engine & session factory builder
        ├── orm.py                  # SQLAlchemy ORM models
        └── repository.py          # TaskRepository + HealthChecker (driven adapters)
```

### Dependency flow

```
routes (driving adapter)
    → ports (protocols)
        ← repository (driven adapter)
            ← orm + database (infrastructure)
```

The domain layer (`models.py`, `ports.py`) has **zero framework
dependencies** — no SQLAlchemy, no FastAPI, no Pydantic. Adapters
implement the ports and can be swapped independently (e.g. tests
use SQLite instead of PostgreSQL).

## Tests

```bash
uv run pytest tests/ -v
```

Three test layers:
- `test_domain.py` — pure domain model tests (no I/O)
- `test_repository.py` — repository adapter tests (SQLite in-memory)
- `test_api.py` — full HTTP integration tests (FastAPI + SQLite)
