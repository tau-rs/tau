# syntax=docker/dockerfile:1.6

# ==========================================================================
# Multi-stage Dockerfile for a Python app managed by uv.
#
# Key ideas:
#   1. Install dependencies in an isolated stage so the layer is cached
#      as long as pyproject.toml / uv.lock don't change.
#   2. Copy the app source in a later stage — code changes don't
#      invalidate the (slow) dependency-install layer.
#   3. Run as a non-root user in production.
# ==========================================================================

# ---------- Stage 1: dependency install ----------
FROM ghcr.io/astral-sh/uv:0.7-python3.12-bookworm-slim AS deps

WORKDIR /app

# Copy only dependency manifests first (layer-cache optimisation).
COPY pyproject.toml uv.lock ./

# Install production dependencies into .venv inside /app.
# --frozen   : fail if uv.lock is out of date (reproducible builds).
# --no-dev   : skip dev/test dependencies.
# --no-install-project : don't install the project itself yet.
RUN uv sync --frozen --no-dev --no-install-project

# ---------- Stage 2: application ----------
FROM ghcr.io/astral-sh/uv:0.7-python3.12-bookworm-slim AS runtime

WORKDIR /app

# Bring the pre-built venv from the deps stage.
COPY --from=deps /app/.venv /app/.venv

# Copy the project source last — only this layer rebuilds on code changes.
COPY pyproject.toml uv.lock ./
COPY src ./src

# Install the project itself (entrypoint / console_scripts) into the
# existing venv without re-resolving or re-downloading deps.
RUN uv sync --frozen --no-dev

# Non-root user for production.
RUN groupadd --system --gid 1000 app \
    && useradd --system --uid 1000 --gid app --shell /usr/sbin/nologin app \
    && chown -R app:app /app
USER app

ENV PATH="/app/.venv/bin:$PATH"
ENV PORT=8000

EXPOSE 8000

ENTRYPOINT ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
