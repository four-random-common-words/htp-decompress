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

  return new Promise((resolve) => {
    setTimeout(() => {
      lastYield = performance.now();
      resolve();
    }, 0);
  });
}
