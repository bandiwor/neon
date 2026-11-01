import { resolve } from 'node:path';

export interface ServerConfig {
  readonly port: number;
  readonly host: string;
  /** The directory that is served. */
  readonly root: string;
  /** Print a line per request. */
  readonly log: boolean;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigError';
  }
}

export const USAGE = `Usage: npm run serve -- [options] [port]

  -p, --port <n>     port to listen on (default 8080, or $PORT; 0 picks a free one)
  -H, --host <name>  address to listen on (default localhost, or $HOST)
  -d, --dir <path>   directory to serve (default dist)
  -q, --quiet        do not log requests
  -h, --help         this text
`;

function parsePort(text: string): number {
  const n = Number(text);
  if (!Number.isInteger(n) || n < 0 || n > 65535) throw new ConfigError(`not a port: ${text}`);
  return n;
}

/** The configuration from the command line and the environment; throws a ConfigError for bad input. */
export function parseConfig(argv: readonly string[], env: Readonly<Record<string, string | undefined>>, cwd: string): ServerConfig {
  let port = env['PORT'] !== undefined ? parsePort(env['PORT']) : 8080;
  let host = env['HOST'] ?? 'localhost';
  let dir = 'dist';
  let log = true;
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i] ?? '';
    const value = (): string => {
      const next = argv[++i];
      if (next === undefined) throw new ConfigError(`${arg} needs a value`);
      return next;
    };
    switch (arg) {
      case '-p':
      case '--port':
        port = parsePort(value());
        break;
      case '-H':
      case '--host':
        host = value();
        break;
      case '-d':
      case '--dir':
        dir = value();
        break;
      case '-q':
      case '--quiet':
        log = false;
        break;
      default:
        if (arg.startsWith('-')) throw new ConfigError(`unknown option ${arg}`);
        port = parsePort(arg);
    }
  }
  return { port, host, root: resolve(cwd, dir), log };
}
