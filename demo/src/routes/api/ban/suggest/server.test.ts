import { afterEach, describe, expect, it } from 'vitest';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { MAX_BAN_SUGGESTIONS } from '$lib/server/ban/schema';
import { POST } from './+server';

const originalBanDataDir = process.env.BAN_DATA_DIR;

afterEach(() => {
  if (originalBanDataDir === undefined) {
    delete process.env.BAN_DATA_DIR;
  } else {
    process.env.BAN_DATA_DIR = originalBanDataDir;
  }
});

describe('POST /api/ban/suggest', () => {
  it('returns bounded BAN suggestions for a query', async () => {
    process.env.BAN_DATA_DIR = missingBanDataDir();

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
    process.env.BAN_DATA_DIR = missingBanDataDir();

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

function missingBanDataDir(): string {
  return join(tmpdir(), 'surch-ban-suggest-missing');
}
