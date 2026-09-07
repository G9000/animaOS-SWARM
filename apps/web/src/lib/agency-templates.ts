import type { PresetId } from './agent-presets';
import type { GeneratedAgency } from './daemon-api';
import type { AccessProfile } from './agent-access';

export interface AgencyMember {
  name: string;
  /** Previous name while an unfinished rename is being edited. Draft only. */
  referenceName?: string;
  bio: string;
  system: string;
  presetId: PresetId;
  provider?: string;
  model?: string;
  suggestedTools?: string[];
  tools?: string[];
  access?: AccessProfile;
}

export function renameTeamReferences(
  text: string,
  renames: ReadonlyArray<readonly [string, string]>,
): string {
  const replacements = new Map(
    renames.map(([before, after]) => [before.trim(), after.trim()] as const)
      .filter(([before, after]) => before && after),
  );
  if (![...replacements].some(([before, after]) => before !== after)) return text;
  // Preserve text the user already corrected, especially when the new full
  // name contains the old short name (Hana -> Aiko Hana). Source names still
  // take precedence so deliberate swaps remain possible.
  for (const after of [...replacements.values()]) {
    if (!replacements.has(after)) replacements.set(after, after);
  }
  const names = [...replacements.keys()]
    .sort((a, b) => b.length - a.length)
    .map((name) => name.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  // Match whole names, including Unicode and possessives. One pass prevents
  // cascading replacements when names are swapped or overlap.
  const pattern = new RegExp(`(?<![\\p{L}\\p{N}_])(?:${names.join('|')})(?![\\p{L}\\p{N}_])`, 'gu');
  return text.replace(pattern, (name) => replacements.get(name) ?? name);
}

/** Add unambiguous first-name references, retaining full names as match guards. */
export function teamNameRenames(
  members: ReadonlyArray<readonly [string, string]>,
): [string, string][] {
  const pairs: [string, string][] = members.map(([before, after]) => [before.trim(), after.trim()]);
  const aliases: [string, string][] = [];
  for (const [index, [before, after]] of pairs.entries()) {
    const parts = before.split(/\s+/);
    if (before === after || parts.length < 2) continue;
    // Role titles are not personal names: never turn "Research findings" into
    // "Alice findings" merely because Research Lead was renamed.
    if (/\b(lead|manager|director|officer|chief|specialist|analyst|researcher|writer|strategist|assistant|engineer|developer|designer|planner|coordinator|reviewer|support)\b/i.test(before)) continue;
    const short = parts[0];
    if (!/^\p{Lu}[\p{L}'’-]*$/u.test(short)) continue;
    const ambiguous = pairs.some(([oldName, newName], other) => other !== index &&
      [oldName, newName].some((name) => name.split(/\s+/)[0].toLowerCase() === short.toLowerCase()));
    if (!ambiguous) aliases.push([short, after]);
  }
  return [...aliases, ...pairs];
}

export interface AgencyTemplate {
  id: string;
  name: string;
  icon: string;
  description: string;
  mission: string;
  values: string[];
  members: AgencyMember[];
  workflow: string[];
  deliverables: string[];
  firstTask: string;
  suggestedConnections: string[];
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
  suggestedTools: ['read_file', 'memory_search', 'todo_read'],
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
    workflow: [
      'Lead confirms the objective, audience, budget, and review owner.',
      'Strategist prepares positioning and a measurable campaign brief.',
      'Copywriter drafts assets; Analyst checks claims and measurement before owner review.',
    ],
    deliverables: [
      'Campaign brief and channel plan',
      'Copy variants with supporting evidence',
      'Measurement plan and experiment backlog',
    ],
    firstTask:
      'Prepare a campaign brief for my main business goal. Ask for the audience, offer, budget, and deadline, then propose one small campaign with draft assets and success measures for my review.',
    suggestedConnections: [
      'Analytics',
      'Content workspace',
      'Social publishing',
    ],
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
    workflow: [
      'Studio Lead confirms voice, audience, platforms, and weekly capacity.',
      'Content Planner proposes themes and an achievable editorial calendar.',
      'Scriptwriter drafts content; Community Manager checks audience relevance before owner review.',
    ],
    deliverables: [
      'Weekly editorial calendar',
      'Scripts and repurposing drafts',
      'Audience feedback summary',
    ],
    firstTask:
      'Prepare a one-week content plan. Ask about my audience, voice, platforms, and available time, then draft three ideas and one complete script for my review.',
    suggestedConnections: [
      'Content library',
      'Social analytics',
      'Community inbox',
    ],
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
    workflow: [
      'Chief of Staff checks priorities, available time, and personal constraints.',
      'Planner turns priorities into realistic next actions and routines.',
      'Research Assistant compares open decisions; the lead prepares an owner-reviewed weekly plan.',
    ],
    deliverables: [
      'Weekly priorities and next actions',
      'Sustainable routine checklist',
      'Decision notes and life admin list',
    ],
    firstTask:
      'Help me prepare a realistic weekly plan. Ask about my top priorities, energy, fixed commitments, and available time, then suggest three priorities and small next actions for my review.',
    suggestedConnections: ['Calendar', 'Personal notes', 'Task list'],
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
  {
    id: 'software-studio',
    name: 'Software Studio',
    icon: '⌘',
    description:
      'Turn a product idea into scoped, reviewed, and tested software.',
    mission:
      'Deliver useful software through clear requirements, small changes, and evidence of quality.',
    values: ['User value', 'Maintainability', 'Verified quality'],
    members: [
      member(
        'Engineering Lead',
        'Clarify the product goal, scope small milestones, assign ownership, and assemble delivery evidence for owner review.',
        'chief-of-staff',
      ),
      member(
        'Product Designer',
        'Describe user journeys, acceptance criteria, interaction states, and accessible layouts; explain design tradeoffs.',
      ),
      member(
        'Developer',
        'Inspect the codebase, propose focused changes, implement approved scope, and document assumptions and integration constraints.',
      ),
      member(
        'Quality Reviewer',
        'Review acceptance criteria, probe failure cases, assess security and regressions, and report reproducible findings with validation evidence.',
        'calm-assistant',
      ),
    ],
    workflow: [
      'Engineering Lead clarifies the user problem and acceptance criteria.',
      'Product Designer and Developer prepare the design and implementation approach.',
      'Developer completes a small increment; Quality Reviewer checks evidence before owner review.',
    ],
    deliverables: [
      'Feature brief and acceptance criteria',
      'Implementation plan and reviewable changes',
      'Validation report and release checklist',
    ],
    firstTask:
      'Prepare a delivery brief for my software idea. Ask who it serves, what problem matters most, and what code already exists, then propose a small first milestone with acceptance criteria and a validation plan.',
    suggestedConnections: [
      'Source repository',
      'Issue tracker',
      'Design files',
    ],
    starter: {
      title: 'Delivery brief',
      content:
        '# Delivery brief\n\n## User problem\nAudience and desired outcome:\n\n## Scope\nIncluded:\nDeferred:\n\n## Acceptance criteria\nObservable behavior and failure states:\n\n## Delivery plan\nChange | Owner | Dependency | Review\n\n## Validation\nTests, accessibility, security, and release evidence:',
    },
  },
  {
    id: 'research',
    name: 'Research Team',
    icon: '⌕',
    description:
      'Investigate questions and turn cited evidence into clear decisions.',
    mission:
      'Produce traceable research that makes uncertainty and decision tradeoffs explicit.',
    values: ['Source quality', 'Intellectual honesty', 'Decision usefulness'],
    members: [
      member(
        'Research Lead',
        'Define the decision, research questions, evidence standard, and stopping criteria; synthesize the final brief.',
        'chief-of-staff',
      ),
      member(
        'Researcher',
        'Find relevant primary sources, record dates and citations, and distinguish reported claims from verified evidence.',
        'calm-assistant',
      ),
      member(
        'Analyst',
        'Compare findings, explain disagreements and limitations, and build a decision matrix with transparent assumptions.',
        'calm-assistant',
      ),
      member(
        'Evidence Reviewer',
        'Check citation support, source freshness, missing perspectives, and overconfident conclusions before owner review.',
        'calm-assistant',
      ),
    ],
    workflow: [
      'Research Lead defines the question, scope, and evidence standard.',
      'Researcher builds a source ledger; Analyst compares findings and gaps.',
      'Evidence Reviewer checks claims and limitations; the lead prepares the decision brief.',
    ],
    deliverables: [
      'Research brief and source ledger',
      'Evidence comparison and uncertainty notes',
      'Decision memo with citations',
    ],
    firstTask:
      'Help me frame my research question. Ask what decision it supports and my constraints, then prepare a research plan, source criteria, and an initial evidence table with explicit unknowns.',
    suggestedConnections: [
      'Document library',
      'Research database',
      'Notes workspace',
    ],
    starter: {
      title: 'Research brief',
      content:
        '# Research brief\n\n## Decision and question\nWhat do we need to learn?\n\n## Scope\nTime period, geography, and exclusions:\n\n## Evidence ledger\nClaim | Source | Date | Support | Limitation\n\n## Comparison\nOption | Evidence | Tradeoff | Confidence\n\n## Open questions\nMissing evidence and next steps:',
    },
  },
  {
    id: 'operations',
    name: 'Operations Team',
    icon: '⚙',
    description:
      'Organize recurring work, improve processes, and make handoffs reliable.',
    mission:
      'Create dependable operations with clear ownership and practical process improvements.',
    values: ['Reliability', 'Clear ownership', 'Continuous improvement'],
    members: [
      member(
        'Operations Lead',
        'Prioritize operational bottlenecks, identify owners and dependencies, and prepare an actionable review agenda.',
        'chief-of-staff',
      ),
      member(
        'Process Designer',
        'Map current workflows, document handoffs and exceptions, and draft lightweight standard operating procedures.',
      ),
      member(
        'Project Coordinator',
        'Break initiatives into milestones and next actions, track dependencies, and flag overdue decisions without creating schedules automatically.',
        'calm-assistant',
      ),
      member(
        'Operations Analyst',
        'Define practical service measures, analyze supplied data, and propose improvements with measurable outcomes.',
        'calm-assistant',
      ),
    ],
    workflow: [
      'Operations Lead identifies the bottleneck and accountable owner.',
      'Process Designer maps the workflow; Project Coordinator identifies dependencies and handoffs.',
      'Operations Analyst proposes measures; the lead submits a small improvement plan for review.',
    ],
    deliverables: [
      'Process map and operating procedure',
      'Ownership and dependency tracker',
      'Improvement plan and review metrics',
    ],
    firstTask:
      'Help me improve one recurring process. Ask how it works today, where it breaks down, and who is involved, then draft a simple operating procedure and one measurable improvement for review.',
    suggestedConnections: ['Project tracker', 'Shared documents', 'Calendar'],
    starter: {
      title: 'Process improvement brief',
      content:
        '# Process improvement brief\n\n## Outcome and bottleneck\nDesired result and current friction:\n\n## Current process\nStep | Owner | Input | Output | Exception\n\n## Proposed improvement\nChange | Benefit | Effort | Dependency\n\n## Handoffs\nWho needs what, and when?\n\n## Review\nMeasure | Baseline | Target | Review owner',
    },
  },
  {
    id: 'customer-support',
    name: 'Customer Support',
    icon: '◎',
    description:
      'Prepare helpful replies, consistent guidance, and actionable customer insights.',
    mission:
      'Help customers get clear answers while improving the underlying support experience.',
    values: ['Empathy', 'Accuracy', 'Customer privacy'],
    members: [
      member(
        'Support Lead',
        'Clarify support policy and priorities, coordinate escalations, and review sensitive or uncertain responses with the owner.',
        'chief-of-staff',
      ),
      member(
        'Support Specialist',
        'Triage supplied requests, draft empathetic evidence-based replies, and identify missing context without promising unauthorized refunds or commitments.',
        'calm-assistant',
      ),
      member(
        'Knowledge Writer',
        'Turn resolved questions into clear help articles and reusable reply drafts, marking policy gaps for review.',
      ),
      member(
        'Customer Insights Analyst',
        'Group recurring issues, distinguish anecdotes from trends, and prepare product feedback with customer details minimized.',
        'calm-assistant',
      ),
    ],
    workflow: [
      'Support Lead confirms policies, escalation criteria, and response priorities.',
      'Support Specialist triages supplied cases and prepares reply drafts.',
      'Knowledge Writer and Insights Analyst capture reusable guidance and trends for owner review.',
    ],
    deliverables: [
      'Triage summary and reviewed reply drafts',
      'Help article and response templates',
      'Recurring issue and escalation report',
    ],
    firstTask:
      'Prepare a support workflow for my business. Ask about the product, common questions, policies, and escalation rules, then draft a triage checklist and three sample replies for my review.',
    suggestedConnections: ['Support inbox', 'Help center', 'Customer records'],
    starter: {
      title: 'Support playbook',
      content:
        '# Support playbook\n\n## Product and policy\nSupported topics and approved commitments:\n\n## Triage\nCase | Urgency | Category | Missing information | Owner\n\n## Reply draft\nAcknowledgment, verified answer, and next step:\n\n## Escalation\nTrigger | Reviewer | Required context\n\n## Learning\nRecurring issue | Help article update | Product feedback',
    },
  },
];

export function templateBrief(template: AgencyTemplate): string {
  return [
    `Goal\n${template.mission}`,
    `What this agency does\n${template.description}`,
    `Expected deliverables\n${template.deliverables.map((item) => `- ${item}`).join('\n')}`,
    `How the team works\n${template.workflow.map((step, index) => `${index + 1}. ${step}`).join('\n')}`,
  ].join('\n\n');
}

export function templateMembers(template: AgencyTemplate): AgencyMember[] {
  return template.members.map((agent, index) => ({
    ...agent,
    suggestedTools: agent.suggestedTools
      ? [...agent.suggestedTools]
      : undefined,
    tools: agent.tools ? [...agent.tools] : undefined,
    system:
      index === 0
        ? `${agent.system}\n\nTeam workflow:\n${template.workflow.map((step, stepIndex) => `${stepIndex + 1}. ${step}`).join('\n')}\n\nExpected deliverables:\n${template.deliverables.map((deliverable) => `- ${deliverable}`).join('\n')}\n\nUse this reusable starter when the owner asks to begin:\n${template.starter.content}`
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
    // Model generation proposes roles, not runtime settings. Only the user's
    // setup selection or explicit per-agent edits may choose a model/provider.
    suggestedTools: Array.isArray(agent.tools) ? [...agent.tools] : undefined,
  }));
  const error = teamError(members[0].name, members.slice(1));
  if (error || !members[0].bio || !members[0].system)
    throw new Error(
      error || 'The generated lead needs a role and instructions. Try again.',
    );
  return members;
}
