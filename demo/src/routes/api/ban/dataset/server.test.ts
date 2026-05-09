import { describe, expect, it } from 'vitest';
import { GET } from './+server';

describe('GET /api/ban/dataset', () => {
  it('returns the active BAN dataset summary and source profile', async () => {
    const response = await GET({} as unknown as Parameters<typeof GET>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.summary).toMatchObject({
      name: 'ban_tiny',
      documentCount: 3
    });
    expect(body.source).toMatchObject({
      kind: 'tiny',
      offline: true
    });
  });
});
