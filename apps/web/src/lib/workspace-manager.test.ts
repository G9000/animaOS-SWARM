import { describe, expect, it } from 'vitest';
import { workspaceManagerProfile } from './workspace-manager';

const context = {
  name: 'Anima',
  companyName: 'Studio',
  mission: 'Create useful content',
  initiative: 'balanced' as const,
  communication: 'concise' as const,
  priorities: '',
  agencyBrief: '',
};

describe('workspaceManagerProfile', () => {
  it('provides a complete manager profile without a model call', () => {
    const profile = workspaceManagerProfile(context);
    expect(profile.bio).toContain('workspace manager');
    expect(profile.system).toContain('You are Anima');
    expect(profile.system).toContain('Studio');
    expect(profile.system).toContain('Create useful content');
    expect(profile.system).toContain('workspace manager');
    expect(profile.system).toContain('tool permissions');
  });

  it('changes initiative without granting permissions or background activity', () => {
    const guided = workspaceManagerProfile({
      ...context,
      initiative: 'guided',
    });
    const proactive = workspaceManagerProfile({
      ...context,
      initiative: 'proactive',
    });
    expect(guided.system).toContain('Ask before changing workspace files');
    expect(proactive.system).toContain('During active work');
    expect(proactive.system).toContain('does not enable background work');
    expect(proactive.system).toContain('tool permissions');
  });

  it('retains the manager role alongside agency context and owner preferences', () => {
    const profile = workspaceManagerProfile({
      ...context,
      communication: 'detailed',
      priorities: 'Use British English',
      agencyBrief: '# Content calendar\nPlan the week',
    });
    expect(profile.system).toContain('workspace manager');
    expect(profile.system).toContain('# Content calendar');
    expect(profile.system).toContain('Use British English');
    expect(profile.style).toContain('context');
  });
});
