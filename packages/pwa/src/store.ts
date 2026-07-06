import type { AppState } from "./types";

type Selector<T> = (state: AppState) => T;
type Subscriber<T> = (next: T, prev: T) => void;

interface Subscription<T> {
  selector: Selector<T>;
  callback: Subscriber<T>;
  lastValue: T;
}

export interface Store {
  get(): AppState;
  update(patch: Partial<AppState>): void;
  subscribe<T>(selector: Selector<T>, callback: Subscriber<T>): () => void;
}

export function createStore(initial: AppState): Store {
  let state = initial;
  const subscriptions: Subscription<unknown>[] = [];

  const queue: Array<Partial<AppState>> = [];
  let processing = false;

  function processQueue() {
    if (processing) return;
    processing = true;
    try {
      while (queue.length > 0) {
        const patch = queue.shift()!;
        const prev = state;
        state = { ...prev, ...patch };
        // Snapshot the subscriber list to avoid mutation-during-iteration.
        const snapshot = subscriptions.slice();
        for (const sub of snapshot) {
          const next = sub.selector(state);
          if (!Object.is(next, sub.lastValue)) {
            const prevValue = sub.lastValue;
            sub.lastValue = next;
            sub.callback(next, prevValue);
          }
        }
      }
    } finally {
      processing = false;
    }
  }

  return {
    get: () => state,
    update(patch) {
      queue.push(patch);
      processQueue();
    },
    subscribe<T>(selector: Selector<T>, callback: Subscriber<T>) {
      const sub: Subscription<T> = {
        selector,
        callback,
        lastValue: selector(state),
      };
      subscriptions.push(sub as Subscription<unknown>);
      return () => {
        const i = subscriptions.indexOf(sub as Subscription<unknown>);
        if (i >= 0) subscriptions.splice(i, 1);
      };
    },
  };
}
