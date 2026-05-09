import { describe, expect, it } from 'vitest';
import { POST } from './+server';

describe('POST /api/count', () => {
  it('returns structured JSON when the upstream body is not JSON', async () => {
    const upstreamBody = `<html>${'not-json'.repeat(100)}</html>`;
    const fakeFetch = async () => new Response(upstreamBody, { status: 200 });

    const response = await POST({
      request: new Request('http://localhost/api/count', {
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
      path: '/ban_tiny/_count',
      upstreamStatus: 200,
      message: 'opensearch /ban_tiny/_count returned a non-JSON response'
    });
    expect(body.body).toContain('<html>');
    expect(body.body.length).toBeLessThan(upstreamBody.length);
    expect(JSON.stringify(body)).not.toContain('SyntaxError');
  });
});
