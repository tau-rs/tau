from __future__ import annotations

from pydantic_settings import BaseSettings


class Settings(BaseSettings):
    database_url: str = "postgresql+asyncpg://app:app@localhost:5432/app"
    port: int = 8000
    app_version: str = "0.1.0"
    echo_sql: bool = False

    model_config = {"env_file": ".env", "env_file_encoding": "utf-8"}
