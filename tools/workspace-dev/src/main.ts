import { getHostDefinition, type HostDefinition, type HostKey } from './hosts.js';
import {
  runManagedProcesses,
  type ManagedProcessDefinition,
} from './process.js';

export interface WorkspaceDevPlan {
  host: HostDefinition;
  processes: ManagedProcessDefinition[];
}

export interface WorkspaceDevPlanOptions {
  reuseExistingHost?: boolean;
}

export function parseHostArg(argv: string[]): HostKey {
  const hostFlagIndex = argv.findIndex((value) => value === '--host');
  const nextValue =
    hostFlagIndex >= 0 ? argv[hostFlagIndex + 1] : undefined;

  if (!nextValue) {
    return 'rust';
  }

  return nextValue as HostKey;
}

export function buildWorkspaceDevPlan(
  hostName: string,
  options: WorkspaceDevPlanOptions = {}
): WorkspaceDevPlan {
  const host = getHostDefinition(hostName);

  if (host.status !== 'ready') {
    throw new Error(
      `Host '${host.key}' is registered as a placeholder and is not implemented yet.`
    );
  }

  const processes: ManagedProcessDefinition[] = [];

  if (!options.reuseExistingHost) {
    processes.push({
      name: host.projectName,
      command: 'bun',
      args: ['x', 'nx', 'run', `${host.projectName}:dev`],
      env: host.env,
    });
  }

  processes.push({
    name: '@animaOS-SWARM/ui',
    command: 'bun',
    args: ['x', 'nx', 'run', '@animaOS-SWARM/ui:serve'],
    env: {
      UI_BACKEND_ORIGIN: host.baseUrl,
      VITE_HOST_KEY: host.key,
    },
  });

  return {
    host,
    processes,
  };
}

export async function run(argv: string[]): Promise<void> {
  const hostName = parseHostArg(argv);
  const host = getHostDefinition(hostName);
  const reuseExistingHost = await isHostReachable(host.baseUrl);
  const plan = buildWorkspaceDevPlan(hostName, { reuseExistingHost });
  await runManagedProcesses(plan.processes);
}

async function isHostReachable(baseUrl: string): Promise<boolean> {
  try {
    const response = await fetch(`${baseUrl}/api/health`);
    return response.ok;
  } catch {
    return false;
  }
}

const isEntrypoint = (
  import.meta as ImportMeta & {
    main?: boolean;
  }
).main === true;

if (isEntrypoint) {
  void run(process.argv.slice(2)).catch((error: unknown) => {
    const message =
      error instanceof Error ? error.message : 'workspace-dev failed';
    console.error(message);
    process.exitCode = 1;
  });
}
