export const PROMPT_LIBRARY = [
  {
    id: 'plan',
    title: 'Plan my next hour',
    category: 'Focus',
    description: 'A realistic plan with one clear outcome.',
    prompt:
      'Help me plan my next hour. Ask what I want to achieve, then help me choose one realistic outcome and break it into small steps.',
  },
  {
    id: 'unblock',
    title: 'Get unstuck',
    category: 'Think',
    description: 'Find the smallest useful next step.',
    prompt:
      'Help me get unstuck. Ask me what is blocking me, explore the assumptions, and suggest the smallest useful next step.',
  },
  {
    id: 'review',
    title: 'Review my work',
    category: 'Improve',
    description: 'A thoughtful second pair of eyes.',
    prompt:
      'Review the work I share next. Look for clarity, correctness, missing details, and practical improvements. Explain the most important changes first.',
  },
  {
    id: 'brainstorm',
    title: 'Explore an idea',
    category: 'Create',
    description: 'Consider three genuinely different directions.',
    prompt:
      'Help me explore an idea. First ask what I want to make and who it is for, then suggest three different approaches with their trade-offs.',
  },
  {
    id: 'summarize',
    title: 'Catch me up',
    category: 'Reflect',
    description: 'Decisions, open questions, and next actions.',
    prompt:
      'Summarize our conversation so far: key decisions, unresolved questions, and the next actions. Distinguish confirmed facts from assumptions.',
  },
  {
    id: 'learn',
    title: 'Learn something deeply',
    category: 'Learn',
    description: 'An explanation that meets you where you are.',
    prompt:
      'Help me understand a topic. Ask what I want to learn and what I already know, then explain it with an example and a short exercise.',
  },
] as const;
