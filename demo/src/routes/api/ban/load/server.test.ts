import { describe, expect, it } from 'vitest';
import { POST } from './+server';

describe('POST /api/ban/load', () => {
  it('returns the tiny BAN bulk payload needed by search engines', async () => {
    const response = await POST({} as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.dataset).toBe('ban_tiny');
    expect(body.bulk).toMatchObject({
      path: '/_bulk',
      contentType: 'application/x-ndjson',
      documentCount: 3
    });
    expect(body.bulk.ndjson).toContain('"Rue de Rivoli"');
  });
});
