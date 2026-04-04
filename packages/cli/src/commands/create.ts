import { Command } from 'commander';
import * as clack from '@clack/prompts';
import pc from 'picocolors';
import { writeFileSync, mkdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import yaml from 'js-yaml';
import { generateAgentTeam, createAdapter } from '../agency/generator.js';
import type { AgencyConfig, AgentDefinition } from '../agency/types.js';
import { PROVIDER_HELP_TEXT } from '../provider-config.js';

export const createCommand = new Command('create')
  .description('Create a new agent agency')
  .argument('[name]', 'Agency name')
  .option('-p, --provider <provider>', PROVIDER_HELP_TEXT, 'openai')
  .option('-m, --model <model>', 'Model to use', 'gpt-4o-mini')
  .option('-d, --description <description>', 'Agency description')
  .option('--api-key <key>', 'API key')
  .option('-y, --yes', 'Accept the generated team without confirmation')
  .action(
    async (
      nameArg: string | undefined,
      opts: {
        provider: string;
        model: string;
        description?: string;
        apiKey?: string;
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

      // 3. Generate the whole team (orchestrator + workers)
      const s = clack.spinner();
      s.start('Building your team...');

      let agents: AgentDefinition[];
      try {
        const adapter = createAdapter(opts.provider, opts.apiKey);
        agents = await generateAgentTeam({
          adapter,
          model: opts.model,
          agencyName: name as string,
          agencyDescription: description as string,
        });
        s.stop(`Generated ${agents.length} agents`);
      } catch (err) {
        s.stop('Failed to generate team');
        clack.log.error(err instanceof Error ? err.message : String(err));
        process.exit(1);
      }

      // 4. Show the team
      const orchestrator =
        agents.find((a) => a.role === 'orchestrator') ?? agents[0];
      const workers = agents.filter((a) => a !== orchestrator);

      clack.log.info(pc.bold('Your team:'));
      clack.log.message(
        `  ${pc.yellow('★')} ${pc.cyan(orchestrator.name)} ${pc.dim(
          '(orchestrator)'
        )} — ${orchestrator.bio}`
      );
      for (const agent of workers) {
        clack.log.message(
          `  ${pc.dim('•')} ${pc.cyan(agent.name)} — ${agent.bio}`
        );
      }

      const accepted = opts.yes
        ? true
        : await clack.confirm({ message: 'Accept this team?' });
      if (clack.isCancel(accepted) || !accepted) {
        clack.cancel('Cancelled.');
        process.exit(0);
      }

      clack.log.info(pc.dim('You can edit the agents later in anima.yaml'));

      // 5. Save
      const dirName = (name as string).toLowerCase().replace(/\s+/g, '-');
      const dirPath = join(process.cwd(), dirName);

      if (!existsSync(dirPath)) {
        mkdirSync(dirPath, { recursive: true });
      }

      const config: AgencyConfig = {
        name: name as string,
        description: description as string,
        model: opts.model,
        provider: opts.provider,
        strategy: 'supervisor',
        orchestrator: {
          name: orchestrator.name,
          bio: orchestrator.bio,
          lore: orchestrator.lore,
          adjectives: orchestrator.adjectives,
          topics: orchestrator.topics,
          knowledge: orchestrator.knowledge,
          style: orchestrator.style,
          system: orchestrator.system,
        },
        agents: workers.map((a) => ({
          name: a.name,
          bio: a.bio,
          lore: a.lore,
          adjectives: a.adjectives,
          topics: a.topics,
          knowledge: a.knowledge,
          style: a.style,
          system: a.system,
        })),
      };

      writeFileSync(
        join(dirPath, 'anima.yaml'),
        yaml.dump(config, { lineWidth: 120, noRefs: true })
      );

      clack.log.success(`Agency saved to ${pc.green(dirName + '/anima.yaml')}`);
      clack.outro(
        `Launch with: ${pc.cyan(`cd ${dirName} && animaos launch "your task"`)}`
      );
    }
  );
