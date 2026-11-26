/** How long decoding may run before it lets the event loop run, in milliseconds; tests change it. */
export const timeSlice = { budgetMs: 8 };

let lastYield = 0;

/**
 * A macrotask to wait for once `timeSlice.budgetMs` has passed since the last yield, so that clicks and aborts get
 * through; undefined while the budget lasts.
 */
export function yieldIfDue(): Promise<void> | undefined {
  if (performance.now() - lastYield < timeSlice.budgetMs) {
    return undefined;
  }

  return yieldToEventLoop().then(() => {
    lastYield = performance.now();
  });
}

/**
 * Lets other tasks run: `scheduler.yield()` where there is one, else a MessageChannel round trip. `setTimeout(0)` is
 * the last resort, since browsers clamp timers nested five deep to 4 ms, and in a continuous decode each yield is
 * nested in the one before.
 */
function yieldToEventLoop(): Promise<void> {
  const { scheduler } = globalThis as { scheduler?: { yield?(): Promise<void> } };

  if (scheduler?.yield !== undefined) {
    return scheduler.yield();
  }

  if (typeof MessageChannel === 'function') {
    return new Promise((resolve) => {
      const channel = new MessageChannel();
      channel.port2.onmessage = () => {
        // Open ports would keep Node's event loop alive.
        channel.port1.close();
        channel.port2.close();
        resolve();
      };
      channel.port1.postMessage(null);
    });
  }

  return new Promise((resolve) => setTimeout(resolve, 0));
}
