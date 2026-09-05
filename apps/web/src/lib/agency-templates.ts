import type { PresetId } from './agent-presets';
import type { GeneratedAgency } from './daemon-api';

export interface AgencyMember {
  name: string;
  bio: string;
  system: string;
  presetId: PresetId;
}

export interface AgencyTemplate {
  id: string;
  name: string;
  icon: string;
  description: string;
  mission: string;
  values: string[];
  members: AgencyMember[];
  starter: { title: string; content: string };
}

const member = (
  name: string,
  bio: string,
  presetId: PresetId = 'creative-partner',
): AgencyMember => ({
  name,
  bio,
  presetId,
  system: `You are the ${name}. ${bio} Ask for missing context, keep drafts actionable, and clearly distinguish facts from assumptions. Prepare work for the owner to review. Do not publish, send messages, or spend money without explicit authorization.`,
});

export const AGENCY_TEMPLATES: AgencyTemplate[] = [
  {
    id: 'marketing',
    name: 'Marketing Agency',
    icon: '↗',
    description:
      'Turn business goals into campaigns, compelling copy, and measurable growth.',
    mission: 'Build clear brand positioning and effective marketing campaigns.',
    values: ['Clarity', 'Customer insight', 'Evidence'],
    members: [
      member(
        'Agency Lead',
        'Coordinate campaign priorities, briefs, and owner reviews.',
        'chief-of-staff',
      ),
      member(
        'Strategist',
        'Define the audience, positioning, campaign objectives, channels, and success measures.',
      ),
      member(
        'Copywriter',
        'Draft on-brand campaign copy, landing pages, emails, and creative variations.',
      ),
      member(
        'Analyst',
        'Review supplied campaign results, explain what changed, and propose measurable experiments.',
        'calm-assistant',
      ),
    ],
    starter: {
      title: 'Campaign brief',
      content:
        '# Campaign brief\n\n## Objective\nWhat business outcome do we want?\n\n## Audience\nWho is this for, and what do they need?\n\n## Message and offer\nPromise, evidence, and call to action.\n\n## Channels and deliverables\nChannel | Asset | Owner | Due date | Review status\n\n## Measurement\nMetric | Baseline | Target | Source\n\n## Launch review\nConfirm claims, budget, approvals, and timing.',
    },
  },
  {
    id: 'creator',
    name: 'Creator Studio',
    icon: '✳',
    description:
      'Plan, create, and repurpose content while staying true to your voice.',
    mission:
      'Create consistent, authentic content and build an engaged community.',
    values: ['Authenticity', 'Consistency', 'Community'],
    members: [
      member(
        'Studio Lead',
        'Coordinate the content pipeline, creative direction, and owner reviews.',
        'chief-of-staff',
      ),
      member(
        'Content Planner',
        'Turn content pillars and audience needs into a realistic editorial calendar.',
      ),
      member(
        'Scriptwriter',
        'Write hooks, scripts, captions, and platform-specific adaptations in the creator’s voice.',
      ),
      member(
        'Community Manager',
        'Draft thoughtful replies, summarize supplied audience feedback, and suggest engagement ideas.',
        'calm-assistant',
      ),
    ],
    starter: {
      title: 'Content calendar',
      content:
        '# Content calendar\n\n## Creator brief\nAudience:\nVoice:\nPlatforms:\nContent pillars:\nWeekly capacity:\n\n## Weekly plan\nDay | Platform | Topic | Hook | Format | Call to action | Status\n\n## Production checklist\nIdea → Outline → Draft → Owner review → Ready to publish\n\n## Repurposing\nSource piece | Short clip | Carousel | Caption\n\n## Weekly review\nWhat resonated? What should we try next?',
    },
  },
  {
    id: 'life',
    name: 'Life Agency',
    icon: '☀',
    description:
      'Make room for what matters with plans, routines, and everyday support.',
    mission:
      'Make steady progress on personal goals with sustainable routines and less admin.',
    values: ['Balance', 'Privacy', 'Sustainable progress'],
    members: [
      member(
        'Personal Chief of Staff',
        'Help prioritize personal goals, coordinate plans, and prepare a weekly review.',
        'chief-of-staff',
      ),
      member(
        'Planner',
        'Break goals into manageable next actions and realistic weekly routines.',
        'calm-assistant',
      ),
      member(
        'Research Assistant',
        'Organize everyday research, compare options from available information, and prepare decisions.',
        'calm-assistant',
      ),
    ],
    starter: {
      title: 'Weekly planning',
      content:
        '# Weekly planning\n\n## Check-in\nEnergy:\nAvailable time:\nWhat matters this week:\n\n## Top three priorities\nPriority | Next action | When | Done\n\n## Routines\nRoutine | Minimum version | Reminder\n\n## Life admin\nTask | Deadline | Information needed\n\n## Weekly reflection\nWins:\nWhat felt difficult:\nOne adjustment for next week:',
    },
  },
];

export function templateMembers(template: AgencyTemplate): AgencyMember[] {
  return template.members.map((agent, index) => ({
    ...agent,
    system:
      index === 0
        ? `${agent.system}\n\nUse this reusable starter when the owner asks to begin:\n${template.starter.content}`
        : agent.system,
  }));
}

export function teamError(
  leadName: string | null,
  workers: AgencyMember[],
): string | null {
  const names = [
    ...(leadName === null ? [] : [leadName]),
    ...workers.map((worker) => worker.name),
  ].map((name) => name.trim().toLowerCase());
  if (names.some((name) => !name)) return 'Every team member needs a name.';
  if (new Set(names).size !== names.length)
    return 'Team member names must be unique.';
  if (workers.some((worker) => !worker.bio.trim() || !worker.system.trim()))
    return 'Every specialist needs a role and instructions.';
  return null;
}

export function generatedMembers(agency: GeneratedAgency): AgencyMember[] {
  if (
    !Array.isArray(agency.agents) ||
    !agency.agents.length ||
    agency.agents.length > 10
  ) {
    throw new Error(
      'Generation must return between 1 and 10 team members. Try again or choose a template.',
    );
  }
  const leadIndex = agency.agents.findIndex(
    (agent) => agent.role === 'orchestrator',
  );
  if (leadIndex < 0)
    throw new Error(
      'The generated team is missing its lead. Try again or choose a template.',
    );
  const ordered = [
    agency.agents[leadIndex],
    ...agency.agents.filter((_, index) => index !== leadIndex),
  ];
  const members: AgencyMember[] = ordered.map((agent, index) => ({
    name: typeof agent.name === 'string' ? agent.name.trim() : '',
    bio: agent.bio?.trim() || agent.position?.trim() || '',
    system: agent.system?.trim() || '',
    presetId: index === 0 ? 'chief-of-staff' : 'creative-partner',
  }));
  const error = teamError(members[0].name, members.slice(1));
  if (error || !members[0].bio || !members[0].system)
    throw new Error(
      error || 'The generated lead needs a role and instructions. Try again.',
    );
  return members;
}
