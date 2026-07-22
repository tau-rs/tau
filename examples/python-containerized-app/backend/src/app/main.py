from __future__ import annotations

from contextlib import asynccontextmanager

from fastapi import FastAPI

from app.adapters.api.routes import router
from app.adapters.persistence.database import build_engine, build_session_factory
from app.adapters.persistence.orm import Base
from app.adapters.persistence.repository import (
    SqlAlchemyHealthChecker,
    SqlAlchemyTaskRepository,
)
from app.config import Settings


def create_app(settings: Settings | None = None) -> FastAPI:
    settings = settings or Settings()

    engine = build_engine(settings)
    session_factory = build_session_factory(engine)

    @asynccontextmanager
    async def lifespan(_app: FastAPI):
        async with engine.begin() as conn:
            await conn.run_sync(Base.metadata.create_all)
        yield
        await engine.dispose()

    application = FastAPI(
        title="python-containerized-app",
        version=settings.app_version,
        lifespan=lifespan,
    )

    application.state.settings = settings
    application.state.task_repo = SqlAlchemyTaskRepository(session_factory)
    application.state.health_checker = SqlAlchemyHealthChecker(session_factory)

    application.include_router(router)
    return application


app = create_app()


def run() -> None:
    import uvicorn

    settings = Settings()
    uvicorn.run(
        "app.main:app",
        host="0.0.0.0",
        port=settings.port,
        reload=False,
    )


if __name__ == "__main__":
    run()
