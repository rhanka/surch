import { describe, expect, it } from 'vitest';
import { MAX_BAN_SUGGESTIONS } from '$lib/server/ban/schema';
import { POST } from './+server';

describe('POST /api/ban/suggest', () => {
  it('returns bounded BAN suggestions for a query', async () => {
    const response = await POST({
      request: new Request('http://localhost/api/ban/suggest', {
        method: 'POST',
        body: JSON.stringify({ query: 'rivo', limit: 5 })
      })
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.limit).toBe(5);
    expect(body.suggestions[0]).toMatchObject({
      id: '75101_0001_00001',
      street_name: 'Rue de Rivoli'
    });
  });

  it('caps requested limits at the backend maximum', async () => {
    const response = await POST({
      request: new Request('http://localhost/api/ban/suggest', {
        method: 'POST',
        body: JSON.stringify({ query: 'rue', limit: 999 })
      })
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.limit).toBe(MAX_BAN_SUGGESTIONS);
    expect(body.suggestions.length).toBeLessThanOrEqual(MAX_BAN_SUGGESTIONS);
  });
});
