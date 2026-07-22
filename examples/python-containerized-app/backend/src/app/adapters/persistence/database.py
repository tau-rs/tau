from __future__ import annotations

from sqlalchemy.ext.asyncio import (
    AsyncEngine,
    async_sessionmaker,
    create_async_engine,
)

from app.config import Settings


def build_engine(settings: Settings) -> AsyncEngine:
    return create_async_engine(settings.database_url, echo=settings.echo_sql)


def build_session_factory(
    engine: AsyncEngine,
) -> async_sessionmaker:
    return async_sessionmaker(engine, expire_on_commit=False)
