import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    globals: true,
    environment: 'node',
    testTimeout: 60000, // 60 seconds - blockchain operations can be slow
    hookTimeout: 30000, // 30 seconds for setup/teardown
    include: ['src/**/*.{test,spec}.{js,mjs,cjs,ts,mts,cts}'],
    exclude: ['node_modules', 'dist'],
    reporters: ['verbose'],
    retry: 0, // Can be increased for flaky blockchain tests
    sequence: {
      shuffle: false, // Run tests in order for blockchain state consistency
    },
  },
});