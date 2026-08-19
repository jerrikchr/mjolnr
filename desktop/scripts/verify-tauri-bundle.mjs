import { readFile } from 'node:fs/promises';

const indexHtml = await readFile(new URL('../build/index.html', import.meta.url), 'utf8');
const tauriConfig = JSON.parse(
  await readFile(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8')
);

const failures = [];
const generatedCsp = indexHtml.match(
  /<meta\s+http-equiv="content-security-policy"\s+content="([^"]+)"/i
)?.[1];
const tauriCsp = tauriConfig?.app?.security?.csp;

if (!generatedCsp?.includes('sha256-')) {
  failures.push('SvelteKit output must hash-authorize its inline bootstrap');
}
if (!generatedCsp?.includes("script-src 'self'")) {
  failures.push("SvelteKit output must restrict scripts to 'self'");
}
if (!indexHtml.includes('kit.start(app, element)')) {
  failures.push('SvelteKit output must contain its client bootstrap');
}
if (typeof tauriCsp !== 'string' || !tauriCsp.includes("script-src 'self' 'unsafe-inline'")) {
  failures.push('Tauri CSP must admit the bootstrap before the generated hash policy narrows it');
}

if (failures.length > 0) {
  for (const failure of failures) {
    process.stderr.write(`bundle verification failed: ${failure}\n`);
  }
  process.exitCode = 1;
} else {
  process.stdout.write('Tauri bundle CSP verification passed\n');
}
