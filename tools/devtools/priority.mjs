// Where a task's priority goes (#1100): the board's one copy of the rule, run by the server (board.mjs,
// on a POST, PATCH or PUT that sets one) and by the page (public/task-board/, which imports it beside
// schema.mjs to show a move at once and write it). Pure functions, no I/O.
//
// A whole number is kept as it is (tasks may share one). A number with a fraction (a move between
// two) slots in: the task takes it rounded up, and the open tasks whose numbers it then runs into
// move down one, as far as the first free number; the rest keep theirs. Tasks sharing a number move
// together. Done, abandoned and archived tasks (archived themselves, or inside an archived folder)
// keep their numbers and are never run into. (While the open numbers run unbroken, a move to the top
// renumbers every one after it.)

const CLOSED = new Set(["done", "abandoned"]);

// `inArchived`: whether a folder it is inside is archived.
export const isOpen = (task, inArchived) => !CLOSED.has(task.status) && !task.archived && !inArchived;

// A task's priority set to `x`, among `others`: the other open tasks, [{id, priority}] (one without a
// number doesn't count). Returns {to: its number, moves: [{id, from, to}], in the order made}.
export function placePriority(x, others) {
  const to = Math.ceil(x);
  const moves = [];
  if (to === x) return { to, moves };
  let floor = to;
  const after = others.filter((o) => typeof o.priority === "number" && Number.isFinite(o.priority) && o.priority > x).sort((a, b) => a.priority - b.priority);
  for (const o of after) {
    if (o.priority > floor) break;
    moves.push({ id: o.id, from: o.priority, to: o.priority + 1 });
    floor = Math.max(floor, o.priority + 1);
  }
  return { to, moves };
}
