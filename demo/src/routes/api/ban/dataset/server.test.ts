import { afterEach, describe, expect, it } from 'vitest';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { GET } from './+server';

const originalBanDataDir = process.env.BAN_DATA_DIR;

afterEach(() => {
  if (originalBanDataDir === undefined) {
    delete process.env.BAN_DATA_DIR;
  } else {
    process.env.BAN_DATA_DIR = originalBanDataDir;
  }
});

describe('GET /api/ban/dataset', () => {
  it('returns the tiny fallback summary when no downloaded BAN CSV is available', async () => {
    process.env.BAN_DATA_DIR = missingBanDataDir();

    const response = await GET({} as unknown as Parameters<typeof GET>[0]);
    const body = await response.json();

    expect(response.status).toBe(200);
    expect(body.summary).toMatchObject({
      name: 'ban_tiny',
      documentCount: 3,
      officialSource: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv'
    });
    expect(body.source).toMatchObject({
      kind: 'tiny',
      offline: true,
      officialUrl: 'https://adresse.data.gouv.fr/data/ban/adresses/latest/csv/adresses-75.csv.gz',
      downloadCommand: 'npm run ban:download'
    });
  });
});

function missingBanDataDir(): string {
  return join(tmpdir(), 'surch-ban-dataset-missing');
}
