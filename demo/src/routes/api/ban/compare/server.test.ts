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

describe('POST /api/ban/compare', () => {
  it('builds an exact label phrase query from the body against the active dataset index', async () => {
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

      return Response.json({
        hits: {
          total: { value: 1, relation: 'eq' },
          hits: [{ _id: '75101_7777_00041' }]
        }
      });
    };

    const response = await POST({
      request: new Request('http://localhost/api/ban/compare', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          id: '75101_7777_00041',
          query: '41 Rue Active 75001 Paris',
          limit: 2
        })
      }),
      fetch: fakeFetch
    } as unknown as Parameters<typeof POST>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.index).toBe('ban_addresses');
    expect(body.summary).toMatchObject({
      name: 'adresses-active.csv',
      documentCount: 1,
      indexName: 'ban_addresses'
    });
    expect(body.query).toMatchObject({
      expectedId: '75101_7777_00041',
      label: 'Exact label: 41 Rue Active 75001 Paris'
    });
    expect(calls.map((call) => [call.method, new URL(call.url).pathname])).toEqual([
      ['POST', '/ban_addresses/_search'],
      ['POST', '/ban_addresses/_search']
    ]);

    const searchBody = JSON.parse(calls[0].body ?? '{}');
    expect(searchBody).toEqual({
      query: {
        match_phrase: {
          label: '41 Rue Active 75001 Paris'
        }
      },
      size: 2,
      track_total_hits: true
    });
    expect(JSON.stringify(searchBody)).not.toContain('Rue de Rivoli');
    expect(body.query.body).toEqual(searchBody);
    expect(body.surch.status).toBe(200);
    expect(body.opensearch.status).toBe(200);
  });
});

function createActiveCsv(fileName: string): string {
  const dir = mkdtempSync(join(tmpdir(), 'surch-ban-compare-'));
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
