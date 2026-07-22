import { Component, inject, OnInit } from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { Task, TaskCreate } from '../../models/task.model';
import { TaskService } from '../../services/task.service';
import { TaskFormComponent } from '../task-form/task-form.component';

@Component({
  selector: 'app-task-list',
  standalone: true,
  imports: [CommonModule, FormsModule, TaskFormComponent],
  template: `
    <div class="container">
      <h1>Tasks</h1>

      <app-task-form (taskCreated)="onCreate($event)" />

      <div class="filters">
        <button [class.active]="filter === 'all'"    (click)="setFilter('all')">All</button>
        <button [class.active]="filter === 'active'" (click)="setFilter('active')">Active</button>
        <button [class.active]="filter === 'done'"   (click)="setFilter('done')">Done</button>
        <span class="count">{{ tasks.length }} task{{ tasks.length === 1 ? '' : 's' }}</span>
      </div>

      @if (tasks.length === 0) {
        <p class="empty">No tasks yet.</p>
      }

      <ul class="task-list">
        @for (task of tasks; track task.id) {
          <li [class.done]="task.done">
            <label class="checkbox-label">
              <input
                type="checkbox"
                [checked]="task.done"
                (change)="onToggle(task)"
              />
              <span class="title">{{ task.title }}</span>
            </label>
            @if (task.description) {
              <span class="desc">{{ task.description }}</span>
            }
            <button class="delete" (click)="onDelete(task)" title="Delete">✕</button>
          </li>
        }
      </ul>
    </div>
  `,
  styles: `
    .container { max-width: 640px; margin: 2rem auto; padding: 0 1rem; }
    h1 { font-size: 1.5rem; font-weight: 600; margin-bottom: 1rem; }
    .filters {
      display: flex; gap: 0.5rem; margin-bottom: 1rem; align-items: center;
    }
    .filters button {
      padding: 0.25rem 0.75rem; border: 1px solid #d1d5db; border-radius: 6px;
      background: white; cursor: pointer; font-size: 0.8rem;
    }
    .filters button.active { background: #2563eb; color: white; border-color: #2563eb; }
    .count { margin-left: auto; font-size: 0.8rem; color: #6b7280; }
    .empty { color: #9ca3af; text-align: center; padding: 2rem 0; }
    .task-list { list-style: none; padding: 0; }
    .task-list li {
      display: flex; align-items: center; gap: 0.5rem;
      padding: 0.75rem; border-bottom: 1px solid #f3f4f6;
    }
    .task-list li:hover { background: #f9fafb; }
    .task-list li.done .title { text-decoration: line-through; color: #9ca3af; }
    .checkbox-label { display: flex; align-items: center; gap: 0.5rem; cursor: pointer; }
    .title { font-size: 0.9rem; }
    .desc { font-size: 0.8rem; color: #6b7280; flex: 1; }
    .delete {
      margin-left: auto; background: none; border: none; color: #ef4444;
      cursor: pointer; font-size: 1rem; padding: 0.25rem; border-radius: 4px;
    }
    .delete:hover { background: #fee2e2; }
  `,
})
export class TaskListComponent implements OnInit {
  private readonly taskService = inject(TaskService);
  tasks: Task[] = [];
  filter: 'all' | 'active' | 'done' = 'all';

  ngOnInit(): void {
    this.loadTasks();
  }

  setFilter(f: 'all' | 'active' | 'done'): void {
    this.filter = f;
    this.loadTasks();
  }

  loadTasks(): void {
    const done =
      this.filter === 'done' ? true : this.filter === 'active' ? false : undefined;
    this.taskService.list(done).subscribe((tasks) => (this.tasks = tasks));
  }

  onCreate(data: TaskCreate): void {
    this.taskService.create(data).subscribe(() => this.loadTasks());
  }

  onToggle(task: Task): void {
    this.taskService.update(task.id, { done: !task.done }).subscribe(() => this.loadTasks());
  }

  onDelete(task: Task): void {
    this.taskService.delete(task.id).subscribe(() => this.loadTasks());
  }
}
