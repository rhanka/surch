import { describe, expect, it } from 'vitest';
import { getBanTinyLoadPayload, parseBanBulkNdjson } from './bulk';

describe('BAN bulk payload', () => {
  it('parses tiny BAN NDJSON action/source pairs', () => {
    const payload = getBanTinyLoadPayload();
    const documents = parseBanBulkNdjson(payload.bulk.ndjson);

    expect(payload.dataset).toBe('ban_tiny');
    expect(payload.bulk.contentType).toBe('application/x-ndjson');
    expect(documents.map((document) => document.id)).toEqual([
      '75101_0001_00001',
      '33063_0002_00010B',
      '67482_0003_00007'
    ]);
  });
});
