import { describe, expect, it } from 'vitest';
import { POST } from './+server';

describe('POST /api/demo/reset', () => {
  it('returns a structured upstream error with the OpenSearch body when reset status differs', async () => {
    const fakeFetch = async (_url: URL | RequestInfo, init?: RequestInit) => {
      if (init?.method === 'DELETE') {
        return Response.json(
          { error: { type: 'index_not_found_exception', reason: 'missing' } },
          { status: 404 }
        );
      }

      return Response.json(
        {
          error: {
            type: 'resource_already_exists_exception',
            reason: 'index [ban_tiny] already exists'
          }
        },
        { status: 400 }
      );
    };

    const response = await POST({
      request: new Request('http://localhost/api/demo/reset', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ engine: 'opensearch' })
      }),
      fetch: fakeFetch
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(502);
    expect(body).toMatchObject({
      error: 'demo_upstream_error',
      status: 502,
      engine: 'opensearch',
      path: '/ban_tiny',
      upstreamStatus: 400,
      message: 'opensearch /ban_tiny expected status 200, got 400',
      body: {
        error: {
          type: 'resource_already_exists_exception'
        }
      }
    });
  });
});
