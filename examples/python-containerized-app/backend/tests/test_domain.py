from __future__ import annotations

from app.domain.models import Task


def test_task_defaults() -> None:
    task = Task(title="Buy milk")
    assert task.title == "Buy milk"
    assert task.description == ""
    assert task.done is False
    assert task.id is None
    assert task.created_at is None


def test_task_all_fields() -> None:
    task = Task(title="X", description="Y", done=True, id=1)
    assert task.id == 1
    assert task.done is True
    assert task.description == "Y"
