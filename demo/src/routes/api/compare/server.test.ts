import { describe, expect, it } from 'vitest';
import { POST } from './+server';

describe('POST /api/compare', () => {
  it('returns a partial result when OpenSearch fails and Surch responds', async () => {
    const fakeFetch = async (url: URL | RequestInfo) => {
      const requestUrl = new URL(url.toString());

      if (requestUrl.origin === 'http://127.0.0.1:9200') {
        throw new Error('connection refused');
      }

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '75101_0001_00001' }]
        }
      });
    };

    const response = await POST({
      request: new Request('http://localhost/api/compare', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ queryId: 'match_label' })
      }),
      fetch: fakeFetch
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.partial).toBe(true);
    expect(body.surch.status).toBe(200);
    expect(body.opensearch).toMatchObject({
      error: 'demo_upstream_error',
      status: 502,
      engine: 'opensearch',
      path: '/ban_tiny/_search',
      message: expect.stringContaining('connection refused')
    });
    expect(body.guardrails.length).toBeGreaterThan(0);
  });
});
