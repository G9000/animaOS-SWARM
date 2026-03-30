// @animaOS-SWARM/tools — tools/index.ts
// Re-exports all tool execution functions and Action objects.

export { executeBash, bashAction } from "./bash.js";
export type { BashArgs } from "./bash.js";

export { executeRead, readAction } from "./read.js";
export type { ReadArgs } from "./read.js";

export { executeWrite, writeAction } from "./write.js";
export type { WriteArgs } from "./write.js";

export { executeEdit, editAction } from "./edit.js";
export type { EditArgs } from "./edit.js";

export { executeMultiEdit, multiEditAction } from "./multi-edit.js";
export type { MultiEditArgs } from "./multi-edit.js";

export { executeGrep, grepAction } from "./grep.js";
export type { GrepArgs } from "./grep.js";

export { executeGlob, globAction } from "./glob.js";
export type { GlobArgs } from "./glob.js";

export { executeListDir, listDirAction } from "./list-dir.js";
export type { ListDirArgs } from "./list-dir.js";

export {
  executeTodoWrite,
  executeTodoRead,
  resetTodos,
  todoWriteAction,
  todoReadAction,
} from "./todo.js";
export type { TodoItem, TodoWriteArgs } from "./todo.js";

export {
  executeBgStart,
  executeBgOutput,
  executeBgStop,
  executeBgList,
  killAllBackground,
  resetProcesses,
  bgStartAction,
  bgOutputAction,
  bgStopAction,
  bgListAction,
} from "./process-manager.js";
export type { BgStartArgs, BgOutputArgs, BgStopArgs } from "./process-manager.js";

export { executeWebFetch, webFetchAction } from "./web-fetch.js";
export type { WebFetchArgs } from "./web-fetch.js";
