import { describe, expect, it } from 'vitest';
import { GET } from './+server';

describe('GET /api/demo/fixture', () => {
  it('returns the BAN fixture and no arbitrary proxy target', async () => {
    const response = await GET();
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.name).toBe('ban_tiny');
    expect(body.documents).toHaveLength(3);
    expect(JSON.stringify(body)).not.toContain('targetUrl');
  });
});
