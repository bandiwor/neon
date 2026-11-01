// A small asynchronous static file server for dist/ (everything else happens in the browser).
//
//   npm run serve [-- --port 8080 --dir dist]
import { existsSync } from 'node:fs';
import { join } from 'node:path';
import { ConfigError, parseConfig, USAGE } from './config';
import { startServer } from './server';

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  if (argv.includes('-h') || argv.includes('--help')) {
    console.log(USAGE);
    return;
  }
  const config = parseConfig(argv, process.env, process.cwd());
  if (!existsSync(join(config.root, 'index.html'))) {
    console.error(`${config.root} has no index.html: run \`npm run build\` first`);
    process.exitCode = 1;
    return;
  }
  const running = await startServer(config);
  console.log(`Neon: ${running.url}  (serving ${config.root})`);
  const stop = (): void => void running.close().then(() => process.exit(0));
  process.on('SIGINT', stop);
  process.on('SIGTERM', stop);
}

main().catch((e: unknown) => {
  console.error(e instanceof ConfigError ? `${e.message}\n\n${USAGE}` : e);
  process.exitCode = 1;
});
