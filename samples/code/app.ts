// Sample TypeScript — syntax highlighting demo.
interface Todo {
  id: number;
  title: string;
  done: boolean;
}

const todos: Todo[] = [
  { id: 1, title: "write code", done: false },
  { id: 2, title: "review PR", done: true },
];

function complete(list: readonly Todo[], id: number): Todo[] {
  return list.map((t) => (t.id === id ? { ...t, done: true } : t));
}

async function load(url: string): Promise<Todo[]> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return (await res.json()) as Todo[];
}

export const result = complete(todos, 1);
console.log(`completed ${result.filter((t) => t.done).length} todo(s)`);
