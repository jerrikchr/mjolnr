import { afterEach } from 'vitest';

/**
 * bits-ui's dialog body-scroll lock restores document.body on a short delayed
 * timer. Let that cleanup finish before Vitest tears down jsdom; otherwise a
 * passing dialog test is reported as an unhandled `document is not defined`.
 */
afterEach(async () => {
  await new Promise((resolve) => setTimeout(resolve, 40));
});
