from __future__ import annotations

from datetime import datetime

from pydantic import BaseModel

from app.domain.models import Task


class TaskCreate(BaseModel):
    title: str
    description: str = ""


class TaskUpdate(BaseModel):
    title: str | None = None
    description: str | None = None
    done: bool | None = None


class TaskOut(BaseModel):
    id: int
    title: str
    description: str
    done: bool
    created_at: datetime

    @classmethod
    def from_domain(cls, task: Task) -> TaskOut:
        return cls(
            id=task.id,  # type: ignore[arg-type]
            title=task.title,
            description=task.description,
            done=task.done,
            created_at=task.created_at,  # type: ignore[arg-type]
        )


class HealthResponse(BaseModel):
    status: str
    timestamp: str
    version: str
    db: str
