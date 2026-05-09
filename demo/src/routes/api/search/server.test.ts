import { describe, expect, it } from 'vitest';
import { POST } from './+server';

describe('POST /api/search', () => {
  it('returns a structured 502 when the engine fetch throws', async () => {
    const fakeFetch = async () => {
      throw new Error('socket closed');
    };

    const response = await POST({
      request: new Request('http://localhost/api/search', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ engine: 'opensearch', queryId: 'match_label' })
      }),
      fetch: fakeFetch
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(502);
    expect(body).toMatchObject({
      error: 'demo_upstream_error',
      status: 502,
      engine: 'opensearch',
      path: '/ban_tiny/_search',
      message: expect.stringContaining('socket closed')
    });
    expect(body.upstreamStatus).toBeUndefined();
  });
});
