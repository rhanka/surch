import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { POST } from './+server';

const originalBanCsvPath = process.env.BAN_CSV_PATH;
const originalBanSampleLimit = process.env.BAN_SAMPLE_LIMIT;
const tempDirs: string[] = [];

afterEach(() => {
  restoreBanEnv();

  while (tempDirs.length > 0) {
    const dir = tempDirs.pop();
    if (dir) {
      rmSync(dir, { force: true, recursive: true });
    }
  }
});

describe('POST /api/ban/load', () => {
  it('loads the active BAN dataset instead of the tiny fixture', async () => {
    const csvPath = createActiveCsv('adresses-active.csv');
    process.env.BAN_CSV_PATH = csvPath;
    process.env.BAN_SAMPLE_LIMIT = '1';
    const calls: Array<{ url: string; method: string; body?: string }> = [];
    const fakeFetch = async (url: URL | RequestInfo, init?: RequestInit) => {
      calls.push({
        url: url.toString(),
        method: init?.method ?? 'GET',
        body: typeof init?.body === 'string' ? init.body : undefined
      });

      return Response.json({ acknowledged: true });
    };

    const response = await POST({
      request: new Request('http://localhost/api/ban/load', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ engines: ['surch'] })
      }),
      fetch: fakeFetch
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.summary).toMatchObject({
      name: 'adresses-active.csv',
      documentCount: 1,
      indexName: 'ban_addresses'
    });
    expect(body.source).toMatchObject({
      kind: 'csv',
      path: csvPath
    });
    expect(body.bulk).toMatchObject({
      path: '/_bulk',
      contentType: 'application/x-ndjson',
      documentCount: 1
    });
    expect(body.bulk.ndjson).toBeUndefined();
    expect(body.index).toBe('ban_addresses');
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['DELETE', '/ban_addresses'],
      ['PUT', '/ban_addresses'],
      ['POST', '/_bulk'],
      ['POST', '/ban_addresses/_refresh']
    ]);
    expect(calls[2].body).toContain('"_index":"ban_addresses"');
    expect(calls[2].body).toContain('"Rue Active"');
    expect(body.engines.surch.operations).toEqual([
      { path: '/ban_addresses', status: 200 },
      { path: '/ban_addresses', status: 200 },
      { path: '/_bulk', status: 200 },
      { path: '/ban_addresses/_refresh', status: 200 }
    ]);
  });

  it('returns a JSON error when the active BAN CSV cannot be loaded', async () => {
    const missingCsvPath = join(tmpdir(), 'surch-ban-load-missing.csv');
    process.env.BAN_CSV_PATH = missingCsvPath;

    const response = await POST({
      request: new Request('http://localhost/api/ban/load', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ engines: ['surch', 'opensearch'] })
      }),
      fetch: async () => Response.json({ acknowledged: true })
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(500);
    expect(body.error).toMatchObject({
      type: 'ban_load_error',
      message: expect.stringContaining('BAN_CSV_PATH does not exist')
    });
    expect(body.error.message).not.toContain(missingCsvPath);
  });
});

function createActiveCsv(fileName: string): string {
  const dir = mkdtempSync(join(tmpdir(), 'surch-ban-load-'));
  tempDirs.push(dir);
  const path = join(dir, fileName);
  writeFileSync(
    path,
    `id,numero,rep,nom_voie,code_postal,code_insee,nom_commune,lon,lat
75101_7777_00041,41,,Rue Active,75001,75101,Paris,2.342,48.859
`
  );

  return path;
}

function restoreBanEnv() {
  if (originalBanCsvPath === undefined) {
    delete process.env.BAN_CSV_PATH;
  } else {
    process.env.BAN_CSV_PATH = originalBanCsvPath;
  }

  if (originalBanSampleLimit === undefined) {
    delete process.env.BAN_SAMPLE_LIMIT;
  } else {
    process.env.BAN_SAMPLE_LIMIT = originalBanSampleLimit;
  }
}
