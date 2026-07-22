export interface Task {
  id: number;
  title: string;
  description: string;
  done: boolean;
  created_at: string;
}

export interface TaskCreate {
  title: string;
  description: string;
}

export interface TaskUpdate {
  title?: string;
  description?: string;
  done?: boolean;
}
