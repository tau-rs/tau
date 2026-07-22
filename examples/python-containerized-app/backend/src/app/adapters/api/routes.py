from __future__ import annotations

from datetime import datetime, timezone

from fastapi import APIRouter, HTTPException, Request

from app.adapters.api.schemas import (
    HealthResponse,
    TaskCreate,
    TaskOut,
    TaskUpdate,
)
from app.domain.ports import HealthChecker, TaskRepository

router = APIRouter()


def _repo(request: Request) -> TaskRepository:
    return request.app.state.task_repo


def _health(request: Request) -> HealthChecker:
    return request.app.state.health_checker


@router.get("/")
def root() -> dict[str, str]:
    return {"app": "python-containerized-app", "docs": "/docs"}


@router.get("/health", response_model=HealthResponse)
async def health(request: Request) -> HealthResponse:
    checker = _health(request)
    db_up = await checker.is_db_up()
    return HealthResponse(
        status="ok",
        timestamp=datetime.now(timezone.utc).isoformat(),
        version=request.app.state.settings.app_version,
        db="ok" if db_up else "unavailable",
    )


@router.post("/tasks", response_model=TaskOut, status_code=201)
async def create_task(body: TaskCreate, request: Request) -> TaskOut:
    repo = _repo(request)
    task = await repo.create(title=body.title, description=body.description)
    return TaskOut.from_domain(task)


@router.get("/tasks", response_model=list[TaskOut])
async def list_tasks(
    request: Request, done: bool | None = None
) -> list[TaskOut]:
    repo = _repo(request)
    tasks = await repo.list_all(done=done)
    return [TaskOut.from_domain(t) for t in tasks]


@router.get("/tasks/{task_id}", response_model=TaskOut)
async def get_task(task_id: int, request: Request) -> TaskOut:
    repo = _repo(request)
    task = await repo.get_by_id(task_id)
    if not task:
        raise HTTPException(404, "Task not found")
    return TaskOut.from_domain(task)


@router.patch("/tasks/{task_id}", response_model=TaskOut)
async def update_task(
    task_id: int, body: TaskUpdate, request: Request
) -> TaskOut:
    repo = _repo(request)
    fields = body.model_dump(exclude_unset=True)
    task = await repo.update(task_id, **fields)
    if not task:
        raise HTTPException(404, "Task not found")
    return TaskOut.from_domain(task)


@router.delete("/tasks/{task_id}", status_code=204)
async def delete_task(task_id: int, request: Request) -> None:
    repo = _repo(request)
    deleted = await repo.delete(task_id)
    if not deleted:
        raise HTTPException(404, "Task not found")
