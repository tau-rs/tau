from __future__ import annotations

from sqlalchemy import select, text
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from app.adapters.persistence.orm import TaskRow
from app.domain.models import Task


def _to_domain(row: TaskRow) -> Task:
    return Task(
        id=row.id,
        title=row.title,
        description=row.description,
        done=row.done,
        created_at=row.created_at,
    )


class SqlAlchemyTaskRepository:
    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._session_factory = session_factory

    async def create(self, title: str, description: str = "") -> Task:
        async with self._session_factory() as session:
            row = TaskRow(title=title, description=description)
            session.add(row)
            await session.commit()
            await session.refresh(row)
            return _to_domain(row)

    async def get_by_id(self, task_id: int) -> Task | None:
        async with self._session_factory() as session:
            row = await session.get(TaskRow, task_id)
            return _to_domain(row) if row else None

    async def list_all(self, *, done: bool | None = None) -> list[Task]:
        async with self._session_factory() as session:
            stmt = select(TaskRow).order_by(TaskRow.created_at.desc())
            if done is not None:
                stmt = stmt.where(TaskRow.done == done)
            result = await session.execute(stmt)
            return [_to_domain(r) for r in result.scalars().all()]

    async def update(self, task_id: int, **fields: object) -> Task | None:
        async with self._session_factory() as session:
            row = await session.get(TaskRow, task_id)
            if not row:
                return None
            for key, value in fields.items():
                setattr(row, key, value)
            await session.commit()
            await session.refresh(row)
            return _to_domain(row)

    async def delete(self, task_id: int) -> bool:
        async with self._session_factory() as session:
            row = await session.get(TaskRow, task_id)
            if not row:
                return False
            await session.delete(row)
            await session.commit()
            return True


class SqlAlchemyHealthChecker:
    def __init__(self, session_factory: async_sessionmaker) -> None:
        self._session_factory = session_factory

    async def is_db_up(self) -> bool:
        try:
            async with self._session_factory() as session:
                await session.execute(text("SELECT 1"))
            return True
        except Exception:
            return False
