import { describe, expect, it } from 'vitest';
import {
  AGENCY_TEMPLATES,
  generatedMembers,
  templateMembers,
  renameTeamReferences,
  teamNameRenames,
} from './agency-templates';
import type { GeneratedAgency } from './daemon-api';

it('updates unique short names while protecting shared names and role titles', () => {
  const renames = teamNameRenames([
    ['Luis Garcia', 'Alice'], ['Hana Sato', 'Aiko Hana'], ['Theo Nguyen', 'Theo Nguyen'],
  ]);
  expect(renameTeamReferences('Luis converts priorities. Ask Luis Garcia and Hana; Theo reviews.', renames))
    .toBe('Alice converts priorities. Ask Alice and Aiko Hana; Theo reviews.');
  expect(renameTeamReferences('Hana works with Aiko Hana.', renames))
    .toBe('Aiko Hana works with Aiko Hana.');
  expect(renameTeamReferences('Luis Garcia and Luis Chen ask Luis.', teamNameRenames([
    ['Luis Garcia', 'Alice'], ['Luis Chen', 'Luis Chen'],
  ]))).toBe('Alice and Luis Chen ask Luis.');
  expect(renameTeamReferences('Research Lead reviews Research findings.', teamNameRenames([
    ['Research Lead', 'Alice'],
  ]))).toBe('Alice reviews Research findings.');
  expect(renameTeamReferences('Luis Garcia and Luis.', teamNameRenames([
    ['Luis Garcia', 'Alice'], ['Bea Chen', 'Luis'],
  ]))).toBe('Alice and Luis.');
});

it('renames whole Unicode names and escaped names in one pass without cascading', () => {
  expect(renameTeamReferences("Amar's draft, Elise’s review; amaranth. 李明：完成。 C++ (Lead).", [
    ['Amar', 'Elise'], ['Elise', 'Amar'], ['李明', '李华'], ['C++ (Lead)', '$Writer'],
  ])).toBe("Elise's draft, Amar’s review; amaranth. 李华：完成。 $Writer.");
  expect(renameTeamReferences('Amar Lee and Amar.', [['Amar', 'Nova'], ['Amar Lee', 'Elise']]))
    .toBe('Elise and Nova.');
});

describe('agency templates', () => {
  it('provides seven complete, useful starting teams without granting access', () => {
    expect(AGENCY_TEMPLATES.map((template) => template.id)).toEqual([
      'marketing',
      'creator',
      'life',
      'software-studio',
      'research',
      'operations',
      'customer-support',
    ]);
    for (const template of AGENCY_TEMPLATES) {
      expect(template.workflow.length).toBeGreaterThan(2);
      expect(template.deliverables.length).toBeGreaterThan(1);
      expect(template.firstTask.length).toBeGreaterThan(30);
      expect(template.suggestedConnections.length).toBeGreaterThan(0);
      expect(
        template.members.every(
          (member) => !member.access && member.suggestedTools?.length,
        ),
      ).toBe(true);
    }
  });

  it('copies editable tool suggestions and gives the manager the workflow and starter', () => {
    const template = AGENCY_TEMPLATES[0];
    const members = templateMembers(template);
    expect(members[0].system).toContain(template.workflow[0]);
    expect(members[0].system).toContain(template.deliverables[0]);
    expect(members[0].system).toContain(template.starter.content);
    members[0].suggestedTools?.push('test-tool');
    expect(template.members[0].suggestedTools).not.toContain('test-tool');
  });
});

describe('generatedMembers', () => {
  it('does not let generated model suggestions override the setup selection', () => {
    const members = generatedMembers({
      name: 'Team',
      provider: 'openai',
      model: 'team-model',
      agents: [
        { name: 'Lead', role: 'orchestrator', bio: 'Lead', system: 'Lead' },
        {
          name: 'Researcher',
          role: 'worker',
          bio: 'Research',
          system: 'Research',
          model: 'research-model',
        },
      ],
    });
    expect(members[1].provider).toBeUndefined();
    expect(members[1].model).toBeUndefined();
  });
  it('orders the lead first and preserves tool suggestions without changing model or access', () => {
    const tools = ['read_file', 'unknown_tool'];
    const agency = {
      name: 'Team',
      agents: [
        {
          name: 'Researcher',
          role: 'worker',
          bio: 'Research',
          system: 'Find evidence',
          provider: 'anthropic',
          model: 'claude',
          tools,
        },
        {
          name: 'Lead',
          role: 'orchestrator',
          bio: 'Coordinate',
          system: 'Coordinate work',
          provider: null,
          model: null,
          tools: null,
        },
      ],
    } as GeneratedAgency;
    const members = generatedMembers(agency);
    expect(members.map((member) => member.name)).toEqual([
      'Lead',
      'Researcher',
    ]);
    expect(members[1]).toMatchObject({
      suggestedTools: tools,
    });
    expect(members.every((member) => member.access === undefined)).toBe(true);
    expect(members.every((member) => member.tools === undefined)).toBe(true);
    expect(members.every((member) => member.provider === undefined)).toBe(true);
    expect(members.every((member) => member.model === undefined)).toBe(true);
    expect(members[0].provider).toBeUndefined();
    expect(members[0].model).toBeUndefined();
    members[1].suggestedTools?.push('write_file');
    expect(tools).toEqual(['read_file', 'unknown_tool']);
  });
});
