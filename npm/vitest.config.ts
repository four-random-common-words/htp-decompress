import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    restoreMocks: true,
    projects: [
      {
        extends: true,
        test: {
          name: 'node',
          include: ['test/**/*.test.ts'],
          exclude: ['test/zone.test.ts']
        }
      },
      {
        extends: true,
        // Lowers async/await to generators on the global Promise, as Angular's build does for an app that uses zone.js.
        oxc: { target: 'es2016' },
        test: {
          name: 'zone',
          include: ['test/zone.test.ts']
        }
      }
    ]
  }
});
