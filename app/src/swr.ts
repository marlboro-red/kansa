import { createResource, createSignal, onCleanup, type Resource } from "solid-js";
import type { Cached } from "./api";

/** Process-wide memory cache so revisiting a view paints instantly before the fetch resolves. */
const mem = new Map<string, unknown>();

/**
 * Bumped by the api client after every mutating core command. `createCached` resources track it,
 * so passive views (sidebar residue badges, repo status, inventory) refetch instead of showing
 * whatever was true when they first mounted.
 */
const [storeVersion, setStoreVersion] = createSignal(0);
export const bumpStoreVersion = () => setStoreVersion((v) => v + 1);

/**
 * Stale-while-revalidate resource over a core `Cached<T>` command: returns memory-cached data
 * immediately, refetches, and keeps polling every `pollMs` while core reports `refreshing`.
 */
export function createSwr<T>(key: () => string, fetcher: (force: boolean) => Promise<Cached<T>>, pollMs = 2000) {
  const [force, setForce] = createSignal(false);
  const [res, { refetch, mutate }] = createResource(
    () => key(),
    async (k) => {
      const c = await fetcher(force());
      setForce(false);
      mem.set(k, c);
      return c;
    },
    { initialValue: mem.get(key()) as Cached<T> | undefined },
  );
  // poll while a background refresh is in flight
  let timer: number | undefined;
  const schedule = () => {
    if (timer) window.clearTimeout(timer);
    if (res.latest?.refreshing) timer = window.setTimeout(() => refetch(), pollMs);
  };
  const stop = () => { if (timer) window.clearTimeout(timer); };
  onCleanup(stop);
  const wrapped = (() => { const v = res(); queueMicrotask(schedule); return v; }) as Resource<Cached<T> | undefined>;
  Object.defineProperty(wrapped, "loading", { get: () => res.loading });
  Object.defineProperty(wrapped, "latest", { get: () => res.latest });
  Object.defineProperty(wrapped, "error", { get: () => res.error });
  Object.defineProperty(wrapped, "state", { get: () => res.state });
  return [wrapped, { refresh: () => { setForce(true); return refetch(); }, refetch, mutate }] as const;
}

/**
 * Memory-first resource: paints the last known value for `key` immediately (from a
 * process-wide map), then fetches and updates. Use for anything the user navigates back to.
 */
const inflight = new Map<string, Promise<unknown>>();

export function createCached<T>(key: () => string | null, fetcher: (k: string) => Promise<T>) {
  const [res, actions] = createResource(
    // The tuple retriggers the fetch when either the key or the store version changes.
    () => { const k = key(); return k === null ? null : ([k, storeVersion()] as const); },
    async ([k, ver]) => {
      // Dedupe concurrent fetches for the same key (several panes ask for repo status at once).
      // The version is part of the dedupe key so a post-mutation refetch never reuses a
      // pre-mutation response that is still in flight.
      const fk = `${k}@${ver}`;
      let pr = inflight.get(fk) as Promise<T> | undefined;
      if (!pr) {
        pr = fetcher(k).finally(() => inflight.delete(fk));
        inflight.set(fk, pr);
      }
      const v = await pr;
      mem.set(k, v);
      return v;
    },
    { initialValue: (key() ? (mem.get(key()!) as T | undefined) : undefined) },
  );
  // When the key changes, seed from memory synchronously so there is no flash of "loading".
  const read = (() => {
    const k = key();
    const v = res();
    if (v === undefined && k && mem.has(k)) return mem.get(k) as T;
    return v;
  }) as Resource<T | undefined>;
  Object.defineProperty(read, "loading", { get: () => res.loading });
  Object.defineProperty(read, "latest", { get: () => res.latest ?? (key() ? (mem.get(key()!) as T | undefined) : undefined) });
  Object.defineProperty(read, "error", { get: () => res.error });
  Object.defineProperty(read, "state", { get: () => res.state });
  return [read, { ...actions, mutate: (v: T) => { if (key()) mem.set(key()!, v); return actions.mutate(() => v); } }] as const;
}

export function ago(iso: string): string {
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  if (s < 5) return "just now";
  if (s < 60) return `${Math.round(s)}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  return `${Math.round(s / 3600)}h ago`;
}
