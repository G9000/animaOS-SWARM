export const HOST_KEYS = ['rust', 'elixir', 'python'] as const;

export type HostKey = (typeof HOST_KEYS)[number];
export type HostStatus = 'ready' | 'placeholder';

export interface HostDefinition {
  key: HostKey;
  projectName: string;
  baseUrl: string;
  status: HostStatus;
  env: Record<string, string>;
}

const HOSTS: Record<HostKey, HostDefinition> = {
  rust: {
    key: 'rust',
    projectName: 'rust-daemon',
    baseUrl: 'http://127.0.0.1:8080',
    status: 'ready',
    env: {
      ANIMAOS_RS_HOST: '127.0.0.1',
      ANIMAOS_RS_PORT: '8080',
    },
  },
  elixir: {
    key: 'elixir',
    projectName: 'elixir-phoenix',
    baseUrl: 'http://127.0.0.1:4100',
    status: 'placeholder',
    env: {},
  },
  python: {
    key: 'python',
    projectName: 'python-service',
    baseUrl: 'http://127.0.0.1:4201',
    status: 'placeholder',
    env: {},
  },
};

export function isHostKey(value: string): value is HostKey {
  return HOST_KEYS.includes(value as HostKey);
}

export function getHostDefinition(value: string): HostDefinition {
  if (!isHostKey(value)) {
    throw new Error(
      `Unknown host '${value}'. Expected one of: ${HOST_KEYS.join(', ')}.`
    );
  }

  return HOSTS[value];
}
