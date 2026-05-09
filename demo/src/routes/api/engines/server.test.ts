import { describe, expect, it } from 'vitest';
import { GET } from './+server';

describe('GET /api/engines', () => {
  it('exposes the supported engine modes only', async () => {
    const response = await GET();
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.modes).toEqual(['surch', 'opensearch', 'compare']);
    expect(body.engines.map((engine: { id: string }) => engine.id)).toEqual([
      'surch',
      'opensearch'
    ]);
  });
});
