import { describe, expect, it } from 'vitest';
import { GET } from './+server';

describe('GET /api/health', () => {
  it('returns demo health without contacting external engines', async () => {
    const response = await GET();
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body).toMatchObject({
      status: 'ok',
      dataset: 'ban_tiny',
      documentCount: 3
    });
  });
});
