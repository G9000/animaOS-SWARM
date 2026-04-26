import { Command } from 'commander';
import * as clack from '@clack/prompts';
import pc from 'picocolors';
import { writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import yaml from 'js-yaml';
import { generateAgentTeam, generateAgentSeeds, createAdapter } from '../agency/generator.js';
import type { AgencyConfig, AgentDefinition } from '../agency/types.js';
import {
  agentSlug,
  renderOrgChart,
  renderAgencyBrief,
  renderAgentProfile,
} from '../agency/diagram.js';
import { PROVIDER_HELP_TEXT } from '../provider-config.js';

export const createCommand = new Command('create')
  .description('Create a new agent agency')
  .argument('[name]', 'Agency name')
  .option('-p, --provider <provider>', PROVIDER_HELP_TEXT, 'openai')
  .option('-m, --model <model>', 'Model to use', 'gpt-4o-mini')
  .option('-d, --description <description>', 'Agency description')
  .option('-s, --size <number>', 'Team size including orchestrator (2-10)', '4')
  .option('--api-key <key>', 'API key')
  .option('--seed', 'Generate starter memories for each agent using the LLM')
  .option('-y, --yes', 'Accept the generated team without confirmation')
  .action(
    async (
      nameArg: string | undefined,
      opts: {
        provider: string;
        model: string;
        description?: string;
        size?: string;
        apiKey?: string;
        seed?: boolean;
        yes?: boolean;
      }
    ) => {
      clack.intro(pc.bgCyan(pc.black(' animaOS-SWARM ')));

      // 1. Agency name
      const name =
        nameArg ??
        (await clack.text({
          message: 'Agency name:',
          placeholder: 'content-team',
          validate: (v) => {
            if (!v.trim()) return 'Name is required';
            return undefined;
          },
        }));
      if (clack.isCancel(name)) {
        clack.cancel('Cancelled.');
        process.exit(0);
      }

      // 2. What does this agency do?
      const description =
        opts.description && opts.description.trim()
          ? opts.description
          : await clack.text({
              message: 'What does this agency do?',
              placeholder: 'Research topics and write high-quality articles',
              validate: (v) =>
                !v.trim() ? 'Description is required' : undefined,
            });
      if (clack.isCancel(description)) {
        clack.cancel('Cancelled.');
        process.exit(0);
      }

      // 3. Team size
      const TEAM_MIN = 2;
      const TEAM_MAX = 10;
      const flagSize = Number(opts.size);
      const sizeFromFlag =
        Number.isFinite(flagSize) &&
        flagSize >= TEAM_MIN &&
        flagSize <= TEAM_MAX
          ? Math.floor(flagSize)
          : undefined;

      const size =
        sizeFromFlag ??
        (await clack.text({
          message: `Team size (including orchestrator, ${TEAM_MIN}-${TEAM_MAX}):`,
          placeholder: '4',
          initialValue: '4',
          validate: (v) => {
            const n = Number(v);
            if (!Number.isFinite(n)) return 'Must be a number';
            if (n < TEAM_MIN || n > TEAM_MAX)
              return `Must be between ${TEAM_MIN} and ${TEAM_MAX}`;
            return undefined;
          },
        }));
      if (clack.isCancel(size)) {
        clack.cancel('Cancelled.');
        process.exit(0);
      }
      const teamSize = typeof size === 'number' ? size : Number(size);

      // 4. Generate the whole team (orchestrator + workers)
      const s = clack.spinner();
      s.start('Building your team...');

      let agents: AgentDefinition[];
      let mission: string | undefined;
      let values: string[] | undefined;
      try {
        const adapter = createAdapter(opts.provider, opts.apiKey);
        const generated = await generateAgentTeam({
          adapter,
          model: opts.model,
          agencyName: name as string,
          agencyDescription: description as string,
          teamSize,
        });
        agents = generated.agents;
        mission = generated.mission;
        values = generated.values;
        s.stop(`Generated ${agents.length} agents`);
      } catch (err) {
        s.stop('Failed to generate team');
        clack.log.error(err instanceof Error ? err.message : String(err));
        process.exit(1);
      }

      // 5. Show mission, values, and the team
      const orchestrator =
        agents.find((a) => a.role === 'orchestrator') ?? agents[0];
      const workers = agents.filter((a) => a !== orchestrator);

      if (mission) {
        clack.log.info(pc.bold('Mission'));
        clack.log.message(`  ${mission}`);
      }
      if (values?.length) {
        clack.log.info(pc.bold('Values'));
        clack.log.message(values.map((v) => `  • ${v}`).join('\n'));
      }

      const formatLine = (a: AgentDefinition, marker: string) => {
        const position = a.position ? pc.dim(` — ${a.position}`) : '';
        const skills = a.tools?.length
          ? `\n      ${pc.dim('skills:')} ${a.tools.join(', ')}`
          : '';
        const collab = a.collaboratesWith?.length
          ? `\n      ${pc.dim('collabs:')} ${a.collaboratesWith.join(', ')}`
          : '';
        return `  ${marker} ${pc.cyan(a.name)}${position}\n      ${a.bio}${skills}${collab}`;
      };

      clack.log.info(pc.bold('Your team:'));
      clack.log.message(formatLine(orchestrator, pc.yellow('★')));
      for (const agent of workers) {
        clack.log.message(formatLine(agent, pc.dim('•')));
      }

      const accepted = opts.yes
        ? true
        : await clack.confirm({ message: 'Accept this team?' });
      if (clack.isCancel(accepted) || !accepted) {
        clack.cancel('Cancelled.');
        process.exit(0);
      }

      clack.log.info(pc.dim('You can edit the agents later in anima.yaml'));

      // 6. Optionally generate seed memories
      const allAgentDefs: AgentDefinition[] = [
        { ...orchestrator, role: 'orchestrator' },
        ...workers.map((a) => ({ ...a, role: 'worker' as const })),
      ];
      let seedsByName = new Map<string, string>();

      if (opts.seed) {
        const ss = clack.spinner();
        ss.start('Generating starter memories...');
        try {
          const adapter = createAdapter(opts.provider, opts.apiKey);
          const seeds = await generateAgentSeeds({
            adapter,
            model: opts.model,
            agencyName: name as string,
            agencyDescription: description as string,
            mission,
            agents: allAgentDefs,
          });
          for (const { agentName, entries } of seeds) {
            if (entries.length > 0) {
              seedsByName.set(agentName, JSON.stringify(entries, null, 2));
            }
          }
          const total = [...seedsByName.values()].reduce(
            (n, v) => n + (JSON.parse(v) as unknown[]).length,
            0
          );
          ss.stop(`Generated ${total} seed memories across ${seedsByName.size} agents`);
        } catch (err) {
          ss.stop('Seed generation failed (skipping)');
          clack.log.warn(err instanceof Error ? err.message : String(err));
        }
      }

      // 7. Save
      const dirName = (name as string).toLowerCase().replace(/\s+/g, '-');
      const dirPath = join(process.cwd(), dirName);

      if (!existsSync(dirPath)) {
        mkdirSync(dirPath, { recursive: true });
      }

      const toAgentDef = (a: AgentDefinition): AgentDefinition => ({
        name: a.name,
        position: a.position,
        bio: a.bio,
        lore: a.lore,
        adjectives: a.adjectives,
        topics: a.topics,
        knowledge: a.knowledge,
        style: a.style,
        system: a.system,
        tools: a.tools,
        collaboratesWith: a.collaboratesWith,
      });

      const config: AgencyConfig = {
        name: name as string,
        description: description as string,
        mission,
        values,
        model: opts.model,
        provider: opts.provider,
        strategy: 'supervisor',
        orchestrator: toAgentDef(orchestrator),
        agents: workers.map(toAgentDef),
      };

      writeFileSync(
        join(dirPath, 'anima.yaml'),
        yaml.dump(config, { lineWidth: 120, noRefs: true })
      );

      writeFileSync(join(dirPath, 'org-chart.mmd'), renderOrgChart(config));
      writeFileSync(join(dirPath, 'README.md'), renderAgencyBrief(config));

      // Per-agent workspace folders — personnel file + placeholders for assets/memory
      const agentsRoot = join(dirPath, 'agents');
      mkdirSync(agentsRoot, { recursive: true });

      for (const agent of allAgentDefs) {
        const slug = agentSlug(agent.name);
        const agentDir = join(agentsRoot, slug);
        mkdirSync(join(agentDir, 'assets'), { recursive: true });
        mkdirSync(join(agentDir, 'memory'), { recursive: true });
        writeFileSync(
          join(agentDir, 'profile.md'),
          renderAgentProfile(agent, name as string)
        );
        writeFileSync(join(agentDir, 'assets', '.gitkeep'), '');
        const seedJson = seedsByName.get(agent.name);
        if (seedJson) {
          writeFileSync(join(agentDir, 'memory', 'seed.json'), seedJson);
        } else {
          writeFileSync(join(agentDir, 'memory', '.gitkeep'), '');
        }
      }

      clack.log.success(`Agency saved to ${pc.green(dirName + '/')}`);
      const seedNote =
        seedsByName.size > 0
          ? `\n  ${pc.dim('•')} ${pc.green('agents/*/memory/seed.json')}  ${pc.dim('— starter memories (' + seedsByName.size + ' agents)')}`
          : '';
      clack.log.message(
        `  ${pc.dim('•')} ${pc.green('anima.yaml')}     ${pc.dim('— team config')}\n` +
          `  ${pc.dim('•')} ${pc.green('org-chart.mmd')}  ${pc.dim('— mermaid diagram')}\n` +
          `  ${pc.dim('•')} ${pc.green('README.md')}      ${pc.dim('— mission, values, roster')}\n` +
          `  ${pc.dim('•')} ${pc.green('agents/')}        ${pc.dim('— personnel folders (' + allAgentDefs.length + ')')}` +
          seedNote
      );
      clack.outro(
        `Launch with: ${pc.cyan(`cd ${dirName} && animaos launch "your task"`)}`
      );
    }
  );
