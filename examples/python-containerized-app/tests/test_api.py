from __future__ import annotations

from httpx import AsyncClient


# ---- Root & Health ----


async def test_root(client: AsyncClient) -> None:
    resp = await client.get("/")
    assert resp.status_code == 200
    body = resp.json()
    assert body["app"] == "python-containerized-app"
    assert "docs" in body


async def test_health(client: AsyncClient) -> None:
    resp = await client.get("/health")
    assert resp.status_code == 200
    body = resp.json()
    assert body["status"] == "ok"
    assert body["db"] == "ok"
    assert "timestamp" in body
    assert "version" in body


# ---- Create ----


async def test_create_task(client: AsyncClient) -> None:
    resp = await client.post(
        "/tasks", json={"title": "Buy milk", "description": "2%"}
    )
    assert resp.status_code == 201
    body = resp.json()
    assert body["title"] == "Buy milk"
    assert body["description"] == "2%"
    assert body["done"] is False
    assert "id" in body
    assert "created_at" in body


async def test_create_task_minimal(client: AsyncClient) -> None:
    resp = await client.post("/tasks", json={"title": "No desc"})
    assert resp.status_code == 201
    assert resp.json()["description"] == ""


async def test_create_task_missing_title(client: AsyncClient) -> None:
    resp = await client.post("/tasks", json={"description": "no title"})
    assert resp.status_code == 422


# ---- List ----


async def test_list_tasks_empty(client: AsyncClient) -> None:
    resp = await client.get("/tasks")
    assert resp.status_code == 200
    assert resp.json() == []


async def test_list_tasks(client: AsyncClient) -> None:
    await client.post("/tasks", json={"title": "A"})
    await client.post("/tasks", json={"title": "B"})
    resp = await client.get("/tasks")
    assert resp.status_code == 200
    tasks = resp.json()
    assert len(tasks) == 2


async def test_list_tasks_filter_done(client: AsyncClient) -> None:
    r1 = await client.post("/tasks", json={"title": "A"})
    task_id = r1.json()["id"]
    await client.patch(f"/tasks/{task_id}", json={"done": True})
    await client.post("/tasks", json={"title": "B"})

    done = await client.get("/tasks", params={"done": "true"})
    assert len(done.json()) == 1
    assert done.json()[0]["done"] is True

    not_done = await client.get("/tasks", params={"done": "false"})
    assert len(not_done.json()) == 1
    assert not_done.json()[0]["done"] is False


# ---- Get ----


async def test_get_task(client: AsyncClient) -> None:
    r = await client.post("/tasks", json={"title": "X"})
    task_id = r.json()["id"]
    resp = await client.get(f"/tasks/{task_id}")
    assert resp.status_code == 200
    assert resp.json()["title"] == "X"


async def test_get_task_not_found(client: AsyncClient) -> None:
    resp = await client.get("/tasks/9999")
    assert resp.status_code == 404


# ---- Update ----


async def test_update_task_title(client: AsyncClient) -> None:
    r = await client.post("/tasks", json={"title": "Old"})
    task_id = r.json()["id"]
    resp = await client.patch(f"/tasks/{task_id}", json={"title": "New"})
    assert resp.status_code == 200
    assert resp.json()["title"] == "New"


async def test_update_task_done(client: AsyncClient) -> None:
    r = await client.post("/tasks", json={"title": "T"})
    task_id = r.json()["id"]
    resp = await client.patch(f"/tasks/{task_id}", json={"done": True})
    assert resp.status_code == 200
    assert resp.json()["done"] is True


async def test_update_task_partial(client: AsyncClient) -> None:
    r = await client.post(
        "/tasks", json={"title": "Keep", "description": "Keep this"}
    )
    task_id = r.json()["id"]
    resp = await client.patch(
        f"/tasks/{task_id}", json={"description": "Changed"}
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["title"] == "Keep"
    assert body["description"] == "Changed"


async def test_update_task_not_found(client: AsyncClient) -> None:
    resp = await client.patch("/tasks/9999", json={"title": "X"})
    assert resp.status_code == 404


# ---- Delete ----


async def test_delete_task(client: AsyncClient) -> None:
    r = await client.post("/tasks", json={"title": "Bye"})
    task_id = r.json()["id"]
    resp = await client.delete(f"/tasks/{task_id}")
    assert resp.status_code == 204

    get_resp = await client.get(f"/tasks/{task_id}")
    assert get_resp.status_code == 404


async def test_delete_task_not_found(client: AsyncClient) -> None:
    resp = await client.delete("/tasks/9999")
    assert resp.status_code == 404


# ---- Full lifecycle ----


async def test_full_lifecycle(client: AsyncClient) -> None:
    r = await client.post(
        "/tasks", json={"title": "Lifecycle", "description": "Full test"}
    )
    assert r.status_code == 201
    task_id = r.json()["id"]

    r = await client.get(f"/tasks/{task_id}")
    assert r.json()["done"] is False

    r = await client.patch(f"/tasks/{task_id}", json={"done": True})
    assert r.json()["done"] is True

    r = await client.get("/tasks", params={"done": "true"})
    assert any(t["id"] == task_id for t in r.json())

    r = await client.delete(f"/tasks/{task_id}")
    assert r.status_code == 204

    r = await client.get("/tasks")
    assert not any(t["id"] == task_id for t in r.json())
