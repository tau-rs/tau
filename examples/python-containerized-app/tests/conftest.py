from __future__ import annotations

from collections.abc import AsyncIterator

import pytest_asyncio
from httpx import ASGITransport, AsyncClient
from sqlalchemy.ext.asyncio import create_async_engine

from app.adapters.persistence.database import build_session_factory
from app.adapters.persistence.orm import Base
from app.adapters.persistence.repository import (
    SqlAlchemyHealthChecker,
    SqlAlchemyTaskRepository,
)
from app.config import Settings
from app.main import create_app

TEST_DB_URL = "sqlite+aiosqlite://"

engine = create_async_engine(TEST_DB_URL)
session_factory = build_session_factory(engine)


@pytest_asyncio.fixture(autouse=True)
async def setup_db():
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.create_all)
    yield
    async with engine.begin() as conn:
        await conn.run_sync(Base.metadata.drop_all)


@pytest_asyncio.fixture
def settings() -> Settings:
    return Settings(database_url=TEST_DB_URL)


@pytest_asyncio.fixture
def repo() -> SqlAlchemyTaskRepository:
    return SqlAlchemyTaskRepository(session_factory)


@pytest_asyncio.fixture
def health_checker() -> SqlAlchemyHealthChecker:
    return SqlAlchemyHealthChecker(session_factory)


@pytest_asyncio.fixture
async def client(settings: Settings) -> AsyncIterator[AsyncClient]:
    application = create_app(settings)
    application.state.task_repo = SqlAlchemyTaskRepository(session_factory)
    application.state.health_checker = SqlAlchemyHealthChecker(session_factory)
    transport = ASGITransport(app=application)
    async with AsyncClient(transport=transport, base_url="http://test") as c:
        yield c
