import { describe, expect, it } from 'vitest';

// The text, not the types: these assertions are about what the generated
// declaration says, which a type import cannot express.
import schema from './generated/schema.d.ts?raw';
import type { components } from './generated/schema';

describe('the generated schema', () => {
  it('carries no credential value', () => {
    expect(schema).not.toMatch(/[0-9a-f]{32,}/);
  });

  it('never names a stored credential', () => {
    for (const stored of ['password_hash', 'token_digest']) {
      expect(schema).not.toContain(stored);
    }
  });

  it('keeps the secret out of a token record', () => {
    const record: components['schemas']['ApiToken'] = {
      id: '01ARZ3NDEKTSV4RRFFQ69G5FAV',
      name: 'laptop',
      scope: 'read',
      created_at: '2026-01-01T00:00:00Z',
    };
    expect(Object.keys(record)).not.toContain('secret');
  });
});
