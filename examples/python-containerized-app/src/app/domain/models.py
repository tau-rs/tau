from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime


@dataclass
class Task:
    title: str
    description: str = ""
    done: bool = False
    id: int | None = field(default=None)
    created_at: datetime | None = field(default=None)
