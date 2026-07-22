import { Component, EventEmitter, Output } from '@angular/core';
import { FormsModule } from '@angular/forms';
import { TaskCreate } from '../../models/task.model';

@Component({
  selector: 'app-task-form',
  standalone: true,
  imports: [FormsModule],
  template: `
    <form (ngSubmit)="onSubmit()" class="task-form">
      <input
        type="text"
        [(ngModel)]="title"
        name="title"
        placeholder="Task title"
        required
      />
      <input
        type="text"
        [(ngModel)]="description"
        name="description"
        placeholder="Description (optional)"
      />
      <button type="submit" [disabled]="!title.trim()">Add</button>
    </form>
  `,
  styles: `
    .task-form {
      display: flex;
      gap: 0.5rem;
      margin-bottom: 1.5rem;
    }
    input {
      padding: 0.5rem 0.75rem;
      border: 1px solid #d1d5db;
      border-radius: 6px;
      font-size: 0.875rem;
    }
    input:first-of-type { flex: 2; }
    input:nth-of-type(2) { flex: 3; }
    button {
      padding: 0.5rem 1.25rem;
      background: #2563eb;
      color: white;
      border: none;
      border-radius: 6px;
      font-weight: 500;
      cursor: pointer;
    }
    button:hover:not(:disabled) { background: #1d4ed8; }
    button:disabled { opacity: 0.5; cursor: not-allowed; }
  `,
})
export class TaskFormComponent {
  @Output() taskCreated = new EventEmitter<TaskCreate>();
  title = '';
  description = '';

  onSubmit(): void {
    if (!this.title.trim()) return;
    this.taskCreated.emit({
      title: this.title.trim(),
      description: this.description.trim(),
    });
    this.title = '';
    this.description = '';
  }
}
