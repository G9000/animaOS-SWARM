import { AGENT_PRESETS, presetById, presetTemplate } from './agent-presets';

describe('agent presets', () => {
  it('ships exactly the four daemon-known presets', () => {
    expect(AGENT_PRESETS.map((preset) => preset.id)).toEqual([
      'chief-of-staff',
      'calm-assistant',
      'senior-engineer',
      'creative-partner',
    ]);
  });

  it('every preset has a label, tagline, and icon', () => {
    for (const preset of AGENT_PRESETS) {
      expect(preset.label.length).toBeGreaterThan(0);
      expect(preset.tagline.length).toBeGreaterThan(0);
      expect(preset.icon.length).toBeGreaterThan(0);
    }
  });

  it('template embeds workspace company and mission', () => {
    const profile = presetTemplate('chief-of-staff', {
      companyName: 'Northwind Research',
      mission: 'Continuous equity research',
      agentName: 'Anima',
    });
    expect(profile.system).toContain('Northwind Research');
    expect(profile.system).toContain('Continuous equity research');
    expect(profile.system).toContain('Anima');
    expect(profile.bio.length).toBeGreaterThan(0);
    expect(profile.adjectives.length).toBe(3);
  });

  it('unknown preset id returns undefined', () => {
    expect(presetById('nope')).toBeUndefined();
  });

  it('every preset template has non-empty bio/style/system and exactly 3 adjectives', () => {
    const context = {
      companyName: 'Northwind Research',
      mission: 'Continuous equity research',
      agentName: 'Anima',
    };
    for (const preset of AGENT_PRESETS) {
      const profile = presetTemplate(preset.id, context);
      expect(profile.bio.length).toBeGreaterThan(0);
      expect(profile.style.length).toBeGreaterThan(0);
      expect(profile.system.length).toBeGreaterThan(0);
      expect(profile.system).toContain(context.companyName);
      expect(profile.system).toContain(context.mission);
      expect(profile.system).toContain(context.agentName);
      expect(profile.bio).toContain(context.companyName);
      expect(profile.adjectives.length).toBe(3);
      for (const adjective of profile.adjectives) {
        expect(adjective.length).toBeGreaterThan(0);
      }
    }
  });
});
