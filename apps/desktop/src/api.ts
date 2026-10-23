import { invoke } from "@tauri-apps/api/core";
export type Mode = "backup" | "mirror" | "bidirectional";
export interface Filter { include_files: string[]; exclude_files: string[]; exclude_directories: string[]; case_insensitive: boolean }
export interface RemoteEndpoint {provider:string; [key:string]:unknown}
export interface Profile { id: string; name: string; group: string; mode: Mode; source: string; destination: string; filter: Filter; allow_delete: boolean; remote_source?:RemoteEndpoint; remote_destination?:RemoteEndpoint }
export interface Observation { kind: string; metadata: { size?: number; modified?: { secs: number; nanos: number } } }
export interface Operation { id: string; path: string; action: string; target?: string; reason: { code: string }; source?: Observation; destination?: Observation; preconditions: unknown[] }
export interface Preview { plan: { id: string; profile: string; mode: Mode; source: { location: string; complete: boolean }; destination: { location: string; complete: boolean }; operations: Operation[] }; summary: Record<string, number> }
export interface Run { execution: string; started: string; state: string; plan: unknown; report?: unknown }
export interface Snapshot { active: boolean; phase: string; profile?: string; entries_done: number; bytes_done: number }
export type Trigger = {kind:"interval"; seconds:number} | {kind:"daily_utc"; hour:number; minute:number};
export interface ScheduleState {schedule:{approved:Profile; trigger:Trigger; allow_deletions:boolean; enabled:boolean}; running_slot?:number; result?:{status:string; code:string}}
export const api = {
  providers: () => invoke<string[]>("providers"),
  schedules: () => invoke<{schedules:ScheduleState[];state:string}>("schedules"),
  scheduleSave: (id:string, trigger:Trigger, skip:boolean, approveDeletions:boolean) => invoke<void>("schedule_save", {id, trigger, missed:skip ? "skip" : "run_once", approveWrites:true, approveDeletions}),
  scheduleRemove: (id:string) => invoke<void>("schedule_remove", {id}),
  profiles: () => invoke<Profile[]>("profiles_list"),
  save: (profile: Profile) => invoke<void>("profile_save", { profile }),
  remove: (id: string) => invoke<void>("profile_remove", { id }),
  planPage: (id:string, offset:number, query:string, action:string) => invoke<{operations:Operation[];total:number}>("plan_page", {id,offset,query,action}),
  preview: (id: string) => invoke<Preview>("preview", { id }),
  execute: (id: string, planId: string, confirmDelete: boolean) => invoke<unknown>("execute", { id, planId, confirm: true, confirmDelete }),
  snapshot: () => invoke<Snapshot>("snapshot"),
  cancel: () => invoke<void>("cancel"),
  historyPage: (id:string, execution:string, offset:number) => invoke<{operations:unknown[];total:number}>("history_page", {id,execution,offset}),
  history: (id: string) => invoke<Run[]>("history", { id }),
  preferences: () => invoke<{ compact: boolean }>("preferences"),
  savePreferences: (compact: boolean) => invoke<void>("preferences_save", { preferences: { compact } }),
};
