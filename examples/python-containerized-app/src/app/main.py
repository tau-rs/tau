from __future__ import annotations

import os
from datetime import datetime, timezone

from fastapi import FastAPI
from pydantic import BaseModel

app = FastAPI(title="python-containerized-app", version="0.1.0")


class HealthResponse(BaseModel):
    status: str
    timestamp: str
    version: str


class GreetRequest(BaseModel):
    name: str


class GreetResponse(BaseModel):
    message: str


@app.get("/health", response_model=HealthResponse)
def health() -> HealthResponse:
    return HealthResponse(
        status="ok",
        timestamp=datetime.now(timezone.utc).isoformat(),
        version=os.getenv("APP_VERSION", "0.1.0"),
    )


@app.post("/greet", response_model=GreetResponse)
def greet(req: GreetRequest) -> GreetResponse:
    return GreetResponse(message=f"Hello, {req.name}!")


@app.get("/")
def root() -> dict[str, str]:
    return {"app": "python-containerized-app", "docs": "/docs"}


def run() -> None:
    import uvicorn

    uvicorn.run(
        "app.main:app",
        host="0.0.0.0",
        port=int(os.getenv("PORT", "8000")),
        reload=os.getenv("ENV", "production") == "development",
    )


if __name__ == "__main__":
    run()
