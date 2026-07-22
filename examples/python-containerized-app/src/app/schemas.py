from __future__ import annotations

from datetime import datetime

from pydantic import BaseModel


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

    model_config = {"from_attributes": True}


class HealthResponse(BaseModel):
    status: str
    timestamp: str
    version: str
    db: str
