from __future__ import annotations

from app.adapters.persistence.repository import (
    SqlAlchemyHealthChecker,
    SqlAlchemyTaskRepository,
)


async def test_create_and_get(repo: SqlAlchemyTaskRepository) -> None:
    task = await repo.create("Buy milk", "2%")
    assert task.id is not None
    assert task.title == "Buy milk"
    assert task.description == "2%"
    assert task.done is False
    assert task.created_at is not None

    fetched = await repo.get_by_id(task.id)
    assert fetched is not None
    assert fetched.title == "Buy milk"


async def test_get_missing(repo: SqlAlchemyTaskRepository) -> None:
    assert await repo.get_by_id(9999) is None


async def test_list_empty(repo: SqlAlchemyTaskRepository) -> None:
    assert await repo.list_all() == []


async def test_list_all(repo: SqlAlchemyTaskRepository) -> None:
    await repo.create("A")
    await repo.create("B")
    tasks = await repo.list_all()
    assert len(tasks) == 2


async def test_list_filter_done(repo: SqlAlchemyTaskRepository) -> None:
    t = await repo.create("A")
    await repo.update(t.id, done=True)  # type: ignore[arg-type]
    await repo.create("B")

    done = await repo.list_all(done=True)
    assert len(done) == 1
    assert done[0].done is True

    not_done = await repo.list_all(done=False)
    assert len(not_done) == 1
    assert not_done[0].done is False


async def test_update(repo: SqlAlchemyTaskRepository) -> None:
    t = await repo.create("Old")
    updated = await repo.update(t.id, title="New", done=True)  # type: ignore[arg-type]
    assert updated is not None
    assert updated.title == "New"
    assert updated.done is True


async def test_update_partial(repo: SqlAlchemyTaskRepository) -> None:
    t = await repo.create("Keep", "Keep this")
    updated = await repo.update(t.id, description="Changed")  # type: ignore[arg-type]
    assert updated is not None
    assert updated.title == "Keep"
    assert updated.description == "Changed"


async def test_update_missing(repo: SqlAlchemyTaskRepository) -> None:
    assert await repo.update(9999, title="X") is None


async def test_delete(repo: SqlAlchemyTaskRepository) -> None:
    t = await repo.create("Bye")
    assert await repo.delete(t.id) is True  # type: ignore[arg-type]
    assert await repo.get_by_id(t.id) is None  # type: ignore[arg-type]


async def test_delete_missing(repo: SqlAlchemyTaskRepository) -> None:
    assert await repo.delete(9999) is False


async def test_health_checker_up(
    health_checker: SqlAlchemyHealthChecker,
) -> None:
    assert await health_checker.is_db_up() is True
