import { createResource, createSignal, onCleanup, type Resource } from "solid-js";
import type { Cached } from "./api";

/** Process-wide memory cache so revisiting a view paints instantly before the fetch resolves. */
const mem = new Map<string, unknown>();

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

export function ago(iso: string): string {
  const s = Math.max(0, (Date.now() - Date.parse(iso)) / 1000);
  if (s < 5) return "just now";
  if (s < 60) return `${Math.round(s)}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  return `${Math.round(s / 3600)}h ago`;
}
