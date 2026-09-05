// Pass only OS necessities to the test server, never a developer's runtime
// configuration, provider credentials, proxies, or persisted control plane.
const systemVariables = new Set([
  'PATH',
  'SYSTEMROOT',
  'WINDIR',
  'TEMP',
  'TMP',
  'TMPDIR',
  'LD_LIBRARY_PATH',
  'DYLD_LIBRARY_PATH',
]);

export function daemonTestEnvironment(
  source: NodeJS.ProcessEnv,
  workspace: string,
  port: number
): NodeJS.ProcessEnv {
  const system = Object.fromEntries(
    Object.entries(source).filter(([key]) =>
      systemVariables.has(key.toUpperCase())
    )
  );
  return {
    ...system,
    ANIMAOS_RS_HOST: '127.0.0.1',
    ANIMAOS_RS_PORT: String(port),
    ANIMAOS_WORKSPACE_ROOT: workspace,
    ANIMAOS_RS_PERSISTENCE_MODE: 'memory',
    ANIMAOS_RS_MEMORY_EMBEDDINGS: 'local',
  };
}
