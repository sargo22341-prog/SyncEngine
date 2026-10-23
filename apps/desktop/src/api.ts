import { invoke } from "@tauri-apps/api/core";
export type Mode = "backup" | "mirror" | "bidirectional";
export interface Filter { include_files: string[]; exclude_files: string[]; exclude_directories: string[]; case_insensitive: boolean }
export interface Profile { id: string; name: string; group: string; mode: Mode; source: string; destination: string; filter: Filter; allow_delete: boolean }
export interface Observation { kind: string; metadata: { size?: number; modified?: { secs: number; nanos: number } } }
export interface Operation { id: string; path: string; action: string; target?: string; reason: { code: string }; source?: Observation; destination?: Observation; preconditions: unknown[] }
export interface Preview { plan: { id: string; profile: string; mode: Mode; source: { location: string; complete: boolean }; destination: { location: string; complete: boolean }; operations: Operation[] }; summary: Record<string, number> }
export interface Run { execution: string; started: string; state: string; plan: unknown; report?: unknown }
export interface Snapshot { active: boolean; phase: string; profile?: string; entries_done: number; bytes_done: number }
export const api = {
  profiles: () => invoke<Profile[]>("profiles_list"),
  save: (profile: Profile) => invoke<void>("profile_save", { profile }),
  remove: (id: string) => invoke<void>("profile_remove", { id }),
  preview: (id: string) => invoke<Preview>("preview", { id }),
  execute: (id: string, planId: string, confirmDelete: boolean) => invoke<unknown>("execute", { id, planId, confirm: true, confirmDelete }),
  snapshot: () => invoke<Snapshot>("snapshot"),
  cancel: () => invoke<void>("cancel"),
  history: (id: string) => invoke<Run[]>("history", { id }),
  preferences: () => invoke<{ compact: boolean }>("preferences"),
  savePreferences: (compact: boolean) => invoke<void>("preferences_save", { preferences: { compact } }),
};
