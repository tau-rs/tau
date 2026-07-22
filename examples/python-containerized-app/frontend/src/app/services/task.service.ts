import { Injectable, inject } from '@angular/core';
import { HttpClient } from '@angular/common/http';
import { Observable } from 'rxjs';
import { Task, TaskCreate, TaskUpdate } from '../models/task.model';

@Injectable({ providedIn: 'root' })
export class TaskService {
  private readonly http = inject(HttpClient);
  private readonly baseUrl = '/api/tasks';

  list(done?: boolean): Observable<Task[]> {
    const params: Record<string, string> = {};
    if (done !== undefined) {
      params['done'] = String(done);
    }
    return this.http.get<Task[]>(this.baseUrl, { params });
  }

  get(id: number): Observable<Task> {
    return this.http.get<Task>(`${this.baseUrl}/${id}`);
  }

  create(task: TaskCreate): Observable<Task> {
    return this.http.post<Task>(this.baseUrl, task);
  }

  update(id: number, changes: TaskUpdate): Observable<Task> {
    return this.http.patch<Task>(`${this.baseUrl}/${id}`, changes);
  }

  delete(id: number): Observable<void> {
    return this.http.delete<void>(`${this.baseUrl}/${id}`);
  }
}
