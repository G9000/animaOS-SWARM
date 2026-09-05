import { expectTypeOf, it } from 'vitest';
import type { AgentSnapshot } from './agents.js';

it('types agent snapshots as JSON descriptors rather than executable runtime objects', () => {
  type Config = AgentSnapshot['state']['config'];
  type Tool = NonNullable<Config['tools']>[number];
  type Settings = NonNullable<Config['settings']>;
  expectTypeOf<Tool>().toHaveProperty('parameters');
  expectTypeOf<Tool>().not.toHaveProperty('handler');
  expectTypeOf<Settings['additional']>().toEqualTypeOf<
    Record<string, unknown>
  >();
  expectTypeOf<Config['bio']>().toEqualTypeOf<string | null>();
  expectTypeOf<AgentSnapshot>().toHaveProperty('messages');
});
